use std::collections::BTreeMap;
use std::io::Write;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, bail, ensure};
use clap::{Parser, Subcommand};
use vr_eval::audio::Voices;
use vr_eval::html::{self, Calibration, Meta};
use vr_eval::judge::{BATCH, Case, Grade, JUDGES, Judge, Verdict, codex_model, needs_key};
use vr_eval::report::{self, Graded};
use vr_eval::run::{Record, Runner};
use vr_eval::{calibrate, scenario};
use vr_radio::config::Config;
use vr_radio::stt;

/// End-to-end evaluation of the voice radio.
#[derive(Parser)]
#[command(name = "vr-eval")]
struct Cli {
    /// Where runs and voices are kept.
    #[arg(long, default_value = "target/eval", global = true)]
    dir: PathBuf,
    /// File with the OpenRouter key (voices, transcription, judges).
    #[arg(long, default_value = ".env", global = true)]
    env_file: PathBuf,
    #[command(subcommand)]
    command: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Runs a scenario set and grades it.
    Run {
        /// Name of the configuration in the reports.
        #[arg(long)]
        name: String,
        #[arg(long, default_value = "crates/vr-eval/scenarios/dev.yaml")]
        set: PathBuf,
        #[arg(long, default_value_t = 1)]
        trials: usize,
        /// Adds trials to the run instead of starting it over, so that
        /// configurations can take turns.
        #[arg(long)]
        append: bool,
        /// Records only; `grade` does the rest later.
        #[arg(long)]
        no_grade: bool,
        /// Types the words instead of saying them: no voices, no
        /// transcription. Times start when the question is sent.
        #[arg(long)]
        text: bool,
        /// Only these stints.
        #[arg(long)]
        stint: Vec<String>,
        /// How the report calls it, and what is special about it.
        #[arg(long)]
        label: Option<String>,
        #[arg(long, default_value = "")]
        note: String,
        /// The `vr` the agent finds on its PATH.
        #[arg(long)]
        vr: Option<PathBuf>,
        /// Options of the radio under test.
        #[arg(last = true)]
        radio: Vec<String>,
    },
    /// Grades (again) the records of a run.
    Grade {
        name: String,
        /// `codex:<model>[@<effort>]` or an OpenRouter model; the default
        /// pair otherwise.
        #[arg(long)]
        judge: Vec<String>,
    },
    /// Measures the judges on replies with known grades.
    Calibrate {
        #[arg(long)]
        judge: Vec<String>,
    },
    /// Summary of runs; two names are also compared.
    Report {
        names: Vec<String>,
        /// Lists the calls that were not good.
        #[arg(long)]
        problems: bool,
    },
    /// One HTML page from runs; the first one of each set is the baseline.
    Html {
        #[arg(required = true)]
        names: Vec<String>,
        #[arg(long)]
        out: Option<PathBuf>,
    },
    /// One stint in its own process (used by `run`).
    #[command(hide = true)]
    Stint {
        #[arg(long)]
        name: String,
        #[arg(long)]
        set: PathBuf,
        #[arg(long)]
        stint: String,
        #[arg(long)]
        trial: usize,
        #[arg(long)]
        port: u16,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        text: bool,
        #[arg(last = true)]
        radio: Vec<String>,
    },
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn".into()),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let key = key(&cli.env_file);
    if matches!(cli.command, Cmd::Stint { .. }) {
        // The agent under test must not inherit the key.
        // SAFETY: no other thread was started yet.
        unsafe { std::env::remove_var("OPENROUTER_API_KEY") };
    }
    match &cli.command {
        Cmd::Run {
            name,
            set,
            trials,
            append,
            no_grade,
            text,
            stint,
            label,
            note,
            vr,
            radio,
        } => {
            let out = cli.dir.join(format!("{name}.jsonl"));
            let first_trial = if *append {
                read::<Record>(&out).map_or(0, |r| r.iter().map(|r| r.trial + 1).max().unwrap_or(0))
            } else {
                let _ = std::fs::remove_file(&out);
                0
            };
            std::fs::create_dir_all(&cli.dir)?;
            let meta = Meta {
                set: set
                    .file_stem()
                    .map(|s| s.to_string_lossy().into())
                    .unwrap_or_default(),
                label: label.clone().unwrap_or_else(|| name.clone()),
                note: note.clone(),
                radio: radio.clone(),
                text: *text,
            };
            std::fs::write(
                cli.dir.join(format!("{name}.meta.json")),
                serde_json::to_vec_pretty(&meta)?,
            )?;
            run(
                &cli,
                name,
                set,
                first_trial..first_trial + *trials,
                stint,
                vr.as_deref(),
                radio,
                *text,
                &out,
            )?;
            if *no_grade {
                return Ok(());
            }
            grade(&cli.dir, name, &[], key.as_deref())?;
            print_report(&cli.dir, std::slice::from_ref(name), false)
        }
        Cmd::Grade { name, judge } => grade(&cli.dir, name, judge, key.as_deref()),
        Cmd::Calibrate { judge } => calibrate_judges(&cli.dir, judge, key.as_deref()),
        Cmd::Html { names, out } => {
            let out = out.clone().unwrap_or_else(|| cli.dir.join("report.html"));
            write_html(&cli.dir, names, &out)
        }
        Cmd::Report { names, problems } => print_report(&cli.dir, names, *problems),
        Cmd::Stint {
            name,
            set,
            stint,
            trial,
            port,
            out,
            text,
            radio,
        } => {
            let stints = scenario::load(set)?;
            let spec = stints
                .iter()
                .find(|s| s.id == *stint)
                .context("unknown stint")?;
            let mut args = vec!["vr-radio".to_string()];
            args.extend(radio.iter().cloned());
            let mut cfg = Config::try_parse_from(args)?;
            cfg.stt_api_key = cfg.stt_api_key.or_else(|| key.clone());
            let runner = Runner {
                config_name: name.clone(),
                trial: *trial,
                voices: Voices {
                    cache: cli.dir.join("voices"),
                    voicevox_url: cfg.voicevox_url.clone(),
                    openrouter_key: key,
                },
                cfg,
                port: *port,
                text: *text,
            };
            let rt = tokio::runtime::Runtime::new()?;
            let records = rt.block_on(runner.stint(spec))?;
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(out)?;
            for r in &records {
                writeln!(file, "{}", serde_json::to_string(r)?)?;
            }
            Ok(())
        }
    }
}

/// The OpenRouter key of the voices, the transcriber and the judges.
fn key(env_file: &Path) -> Option<String> {
    stt::api_key(
        "https://openrouter.ai/api/v1",
        None,
        |n| std::env::var(n).ok(),
        std::fs::read_to_string(env_file).ok().as_deref(),
    )
}

#[allow(clippy::too_many_arguments)]
fn run(
    cli: &Cli,
    name: &str,
    set: &Path,
    trials: std::ops::Range<usize>,
    only: &[String],
    vr: Option<&Path>,
    radio: &[String],
    text: bool,
    out: &Path,
) -> Result<()> {
    let stints = scenario::load(set)?;
    for o in only {
        ensure!(stints.iter().any(|s| s.id == *o), "unknown stint {o}");
    }
    std::fs::create_dir_all(&cli.dir)?;
    let workspace = std::path::absolute(cli.dir.join("workspace"))?;
    std::fs::create_dir_all(&workspace)?;
    let mut path = std::env::var_os("PATH").unwrap_or_default();
    if let Some(vr) = vr {
        let vr = std::path::absolute(vr)?;
        let ok = Command::new(&vr)
            .args(["skill", "install", "--project"])
            .current_dir(&workspace)
            .status()?;
        ensure!(ok.success(), "vr skill install failed");
        let dir = vr.parent().context("vr has no folder")?.to_path_buf();
        path = std::env::join_paths(std::iter::once(dir).chain(std::env::split_paths(&path)))?;
    }
    for trial in trials {
        for s in &stints {
            if !only.is_empty() && !only.contains(&s.id) {
                continue;
            }
            let port = TcpListener::bind("127.0.0.1:0")?.local_addr()?.port();
            let mut args = vec!["--cwd".to_string(), workspace.to_string_lossy().into()];
            if radio.iter().any(|a| a.starts_with("--cwd")) {
                args.clear();
            }
            args.extend(radio.iter().cloned());
            // Once more after a crash of the agent or the network.
            let mut done = false;
            for _ in 0..2 {
                eprintln!("[{name}] trial {trial} stint {}", s.id);
                done = Command::new(std::env::current_exe()?)
                    .arg("--dir")
                    .arg(&cli.dir)
                    .arg("--env-file")
                    .arg(&cli.env_file)
                    .args(["stint", "--name", name, "--stint", &s.id])
                    .arg("--set")
                    .arg(set)
                    .args(["--trial", &trial.to_string(), "--port", &port.to_string()])
                    .arg("--out")
                    .arg(out)
                    .args(text.then_some("--text"))
                    .arg("--")
                    .args(&args)
                    .env("VR_SERVER", format!("http://127.0.0.1:{port}"))
                    .env("PATH", &path)
                    // A nested agent must not think it runs inside another.
                    .env_remove("CLAUDECODE")
                    .env_remove("CLAUDE_CODE_ENTRYPOINT")
                    .status()?
                    .success();
                if done {
                    break;
                }
            }
            if !done {
                bail!("stint {} failed twice", s.id);
            }
        }
    }
    Ok(())
}

fn read<T: serde::de::DeserializeOwned>(path: &Path) -> Result<Vec<T>> {
    let text = std::fs::read_to_string(path).with_context(|| format!("{}", path.display()))?;
    text.lines().map(|l| Ok(serde_json::from_str(l)?)).collect()
}

fn judges(chosen: &[String]) -> Vec<&str> {
    if chosen.is_empty() {
        JUDGES.to_vec()
    } else {
        chosen.iter().map(String::as_str).collect()
    }
}

fn grade(dir: &Path, name: &str, chosen: &[String], key: Option<&str>) -> Result<()> {
    let models = judges(chosen);
    ensure!(
        key.is_some() || !needs_key(&models),
        "the judges need OPENROUTER_API_KEY"
    );
    let records: Vec<Record> = read(&dir.join(format!("{name}.jsonl")))?;
    let cases: Vec<Case> = records.iter().map(Case::from).collect();
    let judged = judge_all(dir, &cases, &models, key)?;
    let mut file = std::fs::File::create(dir.join(format!("{name}.graded.jsonl")))?;
    for (record, verdicts) in records.into_iter().zip(judged) {
        let g = Graded { record, verdicts };
        writeln!(file, "{}", serde_json::to_string(&g)?)?;
    }
    Ok(())
}

/// Every case by every judge, several at a time. A case that one of the
/// judges could not grade gets no verdicts: it stays disputed.
fn judge_all(
    dir: &Path,
    cases: &[Case],
    models: &[&str],
    key: Option<&str>,
) -> Result<Vec<Vec<Verdict>>> {
    let cwd = std::path::absolute(dir.join("judge"))?;
    let mut all: Vec<Vec<Option<Verdict>>> = vec![Vec::new(); cases.len()];
    for m in models {
        // The CLI is a process with its own rate limit.
        let (size, at_once) = if codex_model(m).is_some() {
            (BATCH, 4)
        } else {
            (1, 16)
        };
        let judge = Judge {
            key: key.map(String::from),
            model: m.to_string(),
            cwd: cwd.clone(),
        };
        // Never two replies to one question in a batch: they get mixed up.
        let said: Vec<&str> = cases.iter().map(|c| c.said.as_str()).collect();
        let groups = vr_eval::judge::batches(&said, size);
        let batches: Vec<Vec<Case>> = groups
            .iter()
            .map(|g| g.iter().map(|&i| cases[i].clone()).collect())
            .collect();
        let mut got: Vec<Option<Verdict>> = Vec::new();
        for group in batches.chunks(at_once) {
            let results: Vec<Vec<Option<Verdict>>> = std::thread::scope(|s| {
                let threads: Vec<_> = group
                    .iter()
                    .map(|batch| {
                        let judge = &judge;
                        s.spawn(move || match judge.judge_many(batch) {
                            Ok(v) => v.into_iter().map(Some).collect(),
                            Err(e) => {
                                eprintln!("{m}: {e:#} ({:?})", batch[0].said);
                                vec![None; batch.len()]
                            }
                        })
                    })
                    .collect();
                threads
                    .into_iter()
                    .map(|t| t.join().expect("judge thread"))
                    .collect()
            });
            got.extend(results.into_iter().flatten());
        }
        for (&i, v) in groups.iter().flatten().zip(got) {
            all[i].push(v);
        }
    }
    Ok(all
        .into_iter()
        .map(|v| {
            v.into_iter()
                .collect::<Option<Vec<_>>>()
                .unwrap_or_default()
        })
        .collect())
}

fn write_html(dir: &Path, names: &[String], out: &Path) -> Result<()> {
    let runs = names
        .iter()
        .map(|n| {
            let meta = std::fs::read(dir.join(format!("{n}.meta.json")))
                .ok()
                .and_then(|b| serde_json::from_slice(&b).ok())
                .unwrap_or_else(|| Meta {
                    set: "dev".into(),
                    label: n.clone(),
                    ..Meta::default()
                });
            let rows = read(&dir.join(format!("{n}.graded.jsonl")))?;
            Ok(html::Run { meta, rows })
        })
        .collect::<Result<Vec<_>>>()?;
    let calibration: Vec<Calibration> = std::fs::read(dir.join("calibration.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    std::fs::write(out, html::render(&runs, &calibration, &today()))?;
    println!("{}", out.display());
    Ok(())
}

/// Today's date in UTC (civil-from-days).
fn today() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let z = (secs / 86_400) as i64 + 719_468;
    let era = z / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year}-{month:02}-{day:02}")
}

fn calibrate_judges(dir: &Path, models: &[String], key: Option<&str>) -> Result<()> {
    let labelled = calibrate::cases();
    let cases: Vec<Case> = labelled.iter().map(|l| l.case.clone()).collect();
    let models = judges(models);
    ensure!(
        key.is_some() || !needs_key(&models),
        "the judges need OPENROUTER_API_KEY"
    );
    println!("{} labelled replies", cases.len());
    let mut all: Vec<Vec<Verdict>> = vec![Vec::new(); cases.len()];
    let mut results = Vec::new();
    for (k, model) in models.iter().enumerate() {
        let name = if models.iter().filter(|m| *m == model).count() > 1 {
            format!("{model} #{}", k + 1)
        } else {
            model.to_string()
        };
        let judged = match judge_all(dir, &cases, &[model], key) {
            Ok(j) => j,
            Err(e) => {
                println!("{model}: {e:#}");
                continue;
            }
        };
        let (mut right, mut false_good, mut false_bad) = (0, 0, 0);
        let mut misses = Vec::new();
        for ((l, v), acc) in labelled.iter().zip(&judged).zip(&mut all) {
            let Some(v) = v.first() else { continue };
            let g = v.grade;
            acc.push(v.clone());
            if g == l.label {
                right += 1;
            } else {
                misses.push(format!(
                    "  {} label={:?} got={g:?} {}",
                    l.name, l.label, v.reason
                ));
            }
            false_good += usize::from(g == Grade::Good && l.label != Grade::Good);
            false_bad += usize::from(g != Grade::Good && l.label == Grade::Good);
        }
        println!(
            "{name}: exact {right}/{} false-good {false_good} false-bad {false_bad}",
            cases.len()
        );
        results.push(Calibration {
            model: name,
            cases: cases.len(),
            right,
            false_good,
            false_bad,
            used: JUDGES.contains(model),
        });
        for m in misses {
            println!("{}", m.chars().take(400).collect::<String>());
        }
    }
    let agreed: Vec<_> = labelled
        .iter()
        .zip(&all)
        .filter(|(_, v)| v.len() == models.len())
        .filter_map(|(l, v)| vr_eval::judge::consensus(v).map(|g| (g, l.label)))
        .collect();
    let right = agreed.iter().filter(|(g, l)| g == l).count();
    let unanimous = all
        .iter()
        .filter(|v| v.len() == models.len() && vr_eval::judge::unanimous(v))
        .count();
    println!(
        "together: {} agreed, {right} of them right, {} disputed, {unanimous} unanimous",
        agreed.len(),
        cases.len() - agreed.len()
    );
    if models.len() > 1 {
        // What the reports use: the majority.
        results.push(Calibration {
            model: "多数決".into(),
            cases: cases.len(),
            right,
            false_good: agreed
                .iter()
                .filter(|(g, l)| *g == Grade::Good && *l != Grade::Good)
                .count(),
            false_bad: agreed
                .iter()
                .filter(|(g, l)| *g != Grade::Good && *l == Grade::Good)
                .count(),
            used: true,
        });
    }
    std::fs::create_dir_all(dir)?;
    std::fs::write(
        dir.join("calibration.json"),
        serde_json::to_vec_pretty(&results)?,
    )?;
    Ok(())
}

fn print_report(dir: &Path, names: &[String], problems: bool) -> Result<()> {
    let mut runs = BTreeMap::new();
    for n in names {
        let rows: Vec<Graded> = read(&dir.join(format!("{n}.graded.jsonl")))?;
        runs.insert(n.clone(), rows);
    }
    println!("{}", report::table(&runs));
    if let [a, b] = names {
        println!("{b} minus {a}\n{}", report::compare(&runs[a], &runs[b]));
    }
    if problems {
        for rows in runs.values() {
            println!("{}", report::problems(rows));
        }
    }
    Ok(())
}
