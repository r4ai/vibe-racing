//! Offline speech-to-text with the recognizer built into Windows (SAPI).
//!
//! A resident Windows PowerShell process hosts `System.Speech`: one base64
//! WAV per line in, one JSON line out. Starting it per utterance would add
//! a second to every question.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use serde_json::Value;

use crate::stt::Transcriber;

const TIMEOUT: Duration = Duration::from_secs(30);

const SCRIPT: &str = r#"
$ErrorActionPreference = 'Stop'
$out = [IO.StreamWriter]::new([Console]::OpenStandardOutput(), [Text.UTF8Encoding]::new($false))
$out.AutoFlush = $true
function Reply($o) { $out.WriteLine((ConvertTo-Json $o -Compress)) }
$engine = $null
$failure = $null
try {
  Add-Type -AssemblyName System.Speech
  $info = [Speech.Recognition.SpeechRecognitionEngine]::InstalledRecognizers() |
    Where-Object { '@LANG@' -eq '' -or $_.Culture.TwoLetterISOLanguageName -eq '@LANG@' } |
    Select-Object -First 1
  if (-not $info) { throw 'no Windows speech recognizer is installed for the language "@LANG@"' }
  $engine = [Speech.Recognition.SpeechRecognitionEngine]::new($info)
  $engine.LoadGrammar([Speech.Recognition.DictationGrammar]::new())
} catch { $failure = $_.Exception.Message }
while ($null -ne ($line = [Console]::In.ReadLine())) {
  if ($failure) { Reply @{ error = $failure }; continue }
  try {
    $stream = [IO.MemoryStream]::new([Convert]::FromBase64String($line))
    $engine.SetInputToWaveStream($stream)
    $text = ''
    # A pause ends a result; the end of the audio throws.
    for ($i = 0; $i -lt 50; $i++) {
      try { $r = $engine.Recognize() } catch [InvalidOperationException] { break }
      if ($r) { $text += $r.Text }
    }
    Reply @{ text = $text }
  } catch { Reply @{ error = $_.Exception.Message } }
}
"#;

pub struct WindowsTranscriber {
    program: Program,
    process: Mutex<Option<LineProcess>>,
}

/// How to start the recognizer process.
type Program = Box<dyn Fn() -> Command + Send + Sync>;

impl WindowsTranscriber {
    /// `language` is an ISO-639-1 code; `None` takes any recognizer.
    pub fn new(language: Option<&str>) -> Self {
        let encoded = encode_command(&script(language));
        Self::with_program(Box::new(move || {
            // `System.Speech` exists in Windows PowerShell only, not in pwsh.
            let mut c = Command::new("powershell.exe");
            c.args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded]);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                const CREATE_NO_WINDOW: u32 = 0x0800_0000;
                c.creation_flags(CREATE_NO_WINDOW);
            }
            c
        }))
    }

    fn with_program(program: Program) -> Self {
        Self {
            program,
            process: Mutex::new(None),
        }
    }
}

impl Transcriber for WindowsTranscriber {
    fn transcribe(&self, wav: &[u8]) -> Result<String> {
        let mut slot = self.process.lock().unwrap_or_else(|e| e.into_inner());
        // Taken out: a process that failed is dropped (killed), not reused.
        let mut process = match slot.take() {
            Some(p) => p,
            None => LineProcess::spawn((self.program)())
                .context("cannot start the Windows speech recognizer")?,
        };
        let reply = process.ask(&base64(wav), TIMEOUT)?;
        *slot = Some(process);
        parse_reply(&reply)
    }
}

fn script(language: Option<&str>) -> String {
    // Only letters: the value is pasted into the script.
    let lang: String = language
        .unwrap_or_default()
        .chars()
        .take_while(char::is_ascii_alphabetic)
        .collect();
    SCRIPT.replace("@LANG@", &lang.to_ascii_lowercase())
}

/// `-EncodedCommand` takes base64 of UTF-16LE.
fn encode_command(script: &str) -> String {
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    base64(&utf16)
}

fn parse_reply(line: &str) -> Result<String> {
    let v: Value = serde_json::from_str(line)
        .with_context(|| format!("unexpected answer of the Windows recognizer: {line}"))?;
    if let Some(e) = v["error"].as_str() {
        bail!("Windows speech recognizer: {e}");
    }
    Ok(v["text"].as_str().unwrap_or_default().trim().to_string())
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let mut group = [0u8; 4];
        group[1..1 + chunk.len()].copy_from_slice(chunk);
        let n = u32::from_be_bytes(group);
        for i in 0..4 {
            out.push(if i <= chunk.len() {
                TABLE[(n >> (18 - 6 * i)) as usize & 63] as char
            } else {
                '='
            });
        }
    }
    out
}

/// A child process answering one line per line.
struct LineProcess {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<std::io::Result<String>>,
}

impl LineProcess {
    fn spawn(mut command: Command) -> Result<Self> {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let stdin = child.stdin.take().context("no stdin")?;
        let stdout = child.stdout.take().context("no stdout")?;
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if tx.send(line).is_err() {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            stdin,
            lines,
        })
    }

    fn ask(&mut self, line: &str, timeout: Duration) -> Result<String> {
        self.stdin.write_all(line.as_bytes())?;
        self.stdin.write_all(b"\n")?;
        self.stdin.flush()?;
        let reply = self
            .lines
            .recv_timeout(timeout)
            .map_err(|e| anyhow!("no answer from the Windows speech recognizer: {e}"))?;
        Ok(reply?)
    }
}

impl Drop for LineProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;

    #[test]
    fn base64_matches_rfc_4648() {
        let encoded: Vec<_> = ["", "f", "fo", "foo", "foob", "fooba", "foobar"]
            .iter()
            .map(|s| base64(s.as_bytes()))
            .collect();
        assert_eq!(
            encoded,
            [
                "", "Zg==", "Zm8=", "Zm9v", "Zm9vYg==", "Zm9vYmE=", "Zm9vYmFy"
            ]
        );
        assert_eq!(base64(&[0xfb, 0xff, 0xfe]), "+//+");
        assert_eq!(encode_command("a"), "YQA=");
    }

    #[test]
    fn script_takes_the_language_code_only() {
        assert!(script(Some("ja")).contains("-eq 'ja'"));
        assert!(script(Some("JA-jp")).contains("-eq 'ja'"));
        assert!(script(Some("'; exit #")).contains("'' -eq ''"));
        assert!(script(None).contains("'' -eq ''"));
        assert!(!script(None).contains("@LANG@"));
    }

    #[test]
    fn replies() {
        assert_eq!(parse_reply(r#"{"text":" 燃費は "}"#).unwrap(), "燃費は");
        assert_eq!(parse_reply(r#"{"text":null}"#).unwrap(), "");
        assert_eq!(
            parse_reply(r#"{"error":"x"}"#).unwrap_err().to_string(),
            "Windows speech recognizer: x"
        );
        assert!(parse_reply("oops").is_err());
    }

    /// Answers every line with `reply`.
    fn answering(reply: &'static str, starts: Arc<AtomicUsize>) -> WindowsTranscriber {
        WindowsTranscriber::with_program(Box::new(move || {
            starts.fetch_add(1, Ordering::SeqCst);
            if cfg!(windows) {
                let mut c = Command::new("powershell.exe");
                let script = format!(
                    "while ($null -ne [Console]::In.ReadLine()) {{ [Console]::WriteLine('{reply}') }}"
                );
                c.args(["-NoProfile", "-EncodedCommand", &encode_command(&script)]);
                c
            } else {
                let mut c = Command::new("sh");
                c.args([
                    "-c",
                    &format!("while read -r _; do printf '%s\\n' '{reply}'; done"),
                ]);
                c
            }
        }))
    }

    #[test]
    fn the_process_is_kept_between_questions() {
        let starts = Arc::new(AtomicUsize::new(0));
        let t = answering(r#"{"text":"ok"}"#, starts.clone());
        assert_eq!(t.transcribe(b"a").unwrap(), "ok");
        assert_eq!(t.transcribe(b"b").unwrap(), "ok");
        assert_eq!(starts.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn a_dead_process_is_restarted() {
        let starts = Arc::new(AtomicUsize::new(0));
        let counter = starts.clone();
        let t = WindowsTranscriber::with_program(Box::new(move || {
            counter.fetch_add(1, Ordering::SeqCst);
            // Exits at once.
            let mut c = Command::new(if cfg!(windows) { "cmd" } else { "true" });
            if cfg!(windows) {
                c.args(["/c", "exit"]);
            }
            c
        }));
        assert!(t.transcribe(b"a").is_err());
        assert!(t.transcribe(b"a").is_err());
        assert_eq!(starts.load(Ordering::SeqCst), 2);
        let missing = WindowsTranscriber::with_program(Box::new(|| Command::new("vr-no-such")));
        assert!(
            missing
                .transcribe(b"a")
                .unwrap_err()
                .to_string()
                .starts_with("cannot start the Windows speech recognizer")
        );
    }

    /// The real recognizer, on a WAV named by `VR_RADIO_TEST_WAV`.
    #[test]
    #[ignore = "needs a Windows speech recognizer and a recording"]
    fn recognizes_a_recording() {
        let wav = std::fs::read(std::env::var("VR_RADIO_TEST_WAV").unwrap()).unwrap();
        let t = WindowsTranscriber::new(Some("ja"));
        for _ in 0..2 {
            let t0 = std::time::Instant::now();
            let text = t.transcribe(&wav).unwrap();
            println!("{:?} {text}", t0.elapsed());
            assert!(!text.is_empty());
        }
    }
}
