use std::collections::HashMap;

pub const HEADER_LEN: usize = 112;
pub const VAR_HEADER_LEN: usize = 144;
pub const MAX_BUFS: usize = 4;
pub const STATUS_CONNECTED: i32 = 1;

fn i32_at(b: &[u8], off: usize) -> i32 {
    i32::from_le_bytes(b[off..off + 4].try_into().unwrap())
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    b[..end].iter().map(|&c| c as char).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VarBuf {
    pub tick_count: i32,
    pub offset: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Header {
    pub version: i32,
    pub status: i32,
    pub tick_rate: i32,
    pub session_info_update: i32,
    pub session_info_len: i32,
    pub session_info_offset: i32,
    pub num_vars: i32,
    pub var_header_offset: i32,
    pub num_buf: i32,
    pub buf_len: i32,
    pub bufs: [VarBuf; MAX_BUFS],
}

impl Header {
    pub fn parse(b: &[u8]) -> Option<Self> {
        if b.len() < HEADER_LEN {
            return None;
        }
        let mut bufs = [VarBuf {
            tick_count: 0,
            offset: 0,
        }; MAX_BUFS];
        for (i, vb) in bufs.iter_mut().enumerate() {
            let o = 48 + 16 * i;
            *vb = VarBuf {
                tick_count: i32_at(b, o),
                offset: i32_at(b, o + 4),
            };
        }
        Some(Self {
            version: i32_at(b, 0),
            status: i32_at(b, 4),
            tick_rate: i32_at(b, 8),
            session_info_update: i32_at(b, 12),
            session_info_len: i32_at(b, 16),
            session_info_offset: i32_at(b, 20),
            num_vars: i32_at(b, 24),
            var_header_offset: i32_at(b, 28),
            num_buf: i32_at(b, 32),
            buf_len: i32_at(b, 36),
            bufs,
        })
    }

    pub fn connected(&self) -> bool {
        self.status & STATUS_CONNECTED != 0
    }

    /// The most recently written data buffer.
    pub fn latest_buf(&self) -> Option<(usize, VarBuf)> {
        let n = (self.num_buf.clamp(0, MAX_BUFS as i32)) as usize;
        self.bufs[..n]
            .iter()
            .copied()
            .enumerate()
            .max_by_key(|(_, b)| b.tick_count)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarType {
    Char,
    Bool,
    Int,
    BitField,
    Float,
    Double,
}

impl VarType {
    fn from_i32(v: i32) -> Option<Self> {
        Some(match v {
            0 => Self::Char,
            1 => Self::Bool,
            2 => Self::Int,
            3 => Self::BitField,
            4 => Self::Float,
            5 => Self::Double,
            _ => return None,
        })
    }

    pub fn size(self) -> usize {
        match self {
            Self::Char | Self::Bool => 1,
            Self::Int | Self::BitField | Self::Float => 4,
            Self::Double => 8,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct VarHeader {
    pub ty: VarType,
    pub offset: usize,
    pub count: usize,
    pub name: String,
    pub unit: String,
}

#[derive(Debug, Clone, Default)]
pub struct VarTable {
    vars: HashMap<String, VarHeader>,
}

impl VarTable {
    pub fn parse(b: &[u8], num_vars: usize) -> Self {
        let mut vars = HashMap::with_capacity(num_vars);
        for i in 0..num_vars {
            let o = i * VAR_HEADER_LEN;
            if o + VAR_HEADER_LEN > b.len() {
                break;
            }
            let h = &b[o..o + VAR_HEADER_LEN];
            let Some(ty) = VarType::from_i32(i32_at(h, 0)) else {
                continue;
            };
            let name = cstr(&h[16..48]);
            vars.insert(
                name.clone(),
                VarHeader {
                    ty,
                    offset: i32_at(h, 4).max(0) as usize,
                    count: i32_at(h, 8).max(0) as usize,
                    name,
                    unit: cstr(&h[112..144]),
                },
            );
        }
        Self { vars }
    }

    pub fn len(&self) -> usize {
        self.vars.len()
    }

    pub fn is_empty(&self) -> bool {
        self.vars.is_empty()
    }

    pub fn get(&self, name: &str) -> Option<&VarHeader> {
        self.vars.get(name)
    }

    /// Bind the table to one data buffer for typed access.
    pub fn view<'a>(&'a self, data: &'a [u8]) -> VarView<'a> {
        VarView { table: self, data }
    }
}

/// Typed access to one data buffer. Every accessor returns `None` when the
/// variable does not exist (older sim versions) or is out of range.
pub struct VarView<'a> {
    table: &'a VarTable,
    data: &'a [u8],
}

impl VarView<'_> {
    pub fn f64_at(&self, name: &str, idx: usize) -> Option<f64> {
        let v = self.table.get(name)?;
        if idx >= v.count.max(1) {
            return None;
        }
        let o = v.offset + idx * v.ty.size();
        let b = self.data.get(o..o + v.ty.size())?;
        Some(match v.ty {
            VarType::Char => b[0] as f64,
            VarType::Bool => (b[0] != 0) as i32 as f64,
            VarType::Int | VarType::BitField => i32::from_le_bytes(b.try_into().ok()?) as f64,
            VarType::Float => f32::from_le_bytes(b.try_into().ok()?) as f64,
            VarType::Double => f64::from_le_bytes(b.try_into().ok()?),
        })
    }

    pub fn f64(&self, name: &str) -> Option<f64> {
        self.f64_at(name, 0).filter(|v| v.is_finite())
    }

    /// Integer view of any numeric variable (floats truncate; every i32 is
    /// exactly representable as f64, so ints and bitfields round-trip).
    pub fn i32_at(&self, name: &str, idx: usize) -> Option<i32> {
        self.f64_at(name, idx).map(|x| x as i32)
    }

    pub fn i32(&self, name: &str) -> Option<i32> {
        self.i32_at(name, 0)
    }

    pub fn u32(&self, name: &str) -> Option<u32> {
        self.i32(name).map(|v| v as u32)
    }

    pub fn bool_at(&self, name: &str, idx: usize) -> Option<bool> {
        self.f64_at(name, idx).map(|v| v != 0.0)
    }

    pub fn bool(&self, name: &str) -> Option<bool> {
        self.bool_at(name, 0)
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    //! Builds a fake iRacing memory image for tests.
    use super::*;

    pub struct Var {
        pub name: &'static str,
        pub ty: i32,
        pub count: usize,
    }

    pub struct Image {
        pub bytes: Vec<u8>,
        pub offsets: HashMap<&'static str, (usize, usize)>,
        pub buf_offset: usize,
    }

    impl Image {
        pub fn new(vars: &[Var], yaml: &str) -> Self {
            let var_hdr_off = HEADER_LEN;
            let yaml_off = var_hdr_off + vars.len() * VAR_HEADER_LEN;
            let buf_off = yaml_off + yaml.len() + 1;
            let mut offsets = HashMap::new();
            let mut cur = 0usize;
            let mut hdrs = Vec::new();
            for v in vars {
                let size = VarType::from_i32(v.ty).unwrap().size();
                offsets.insert(v.name, (cur, size));
                let mut h = vec![0u8; VAR_HEADER_LEN];
                h[0..4].copy_from_slice(&v.ty.to_le_bytes());
                h[4..8].copy_from_slice(&(cur as i32).to_le_bytes());
                h[8..12].copy_from_slice(&(v.count as i32).to_le_bytes());
                h[16..16 + v.name.len()].copy_from_slice(v.name.as_bytes());
                hdrs.extend(h);
                cur += size * v.count;
            }
            let buf_len = cur;
            let mut b = vec![0u8; buf_off + buf_len];
            let w =
                |b: &mut Vec<u8>, o: usize, v: i32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
            w(&mut b, 0, 2);
            w(&mut b, 4, 1);
            w(&mut b, 8, 60);
            w(&mut b, 12, 1);
            w(&mut b, 16, yaml.len() as i32);
            w(&mut b, 20, yaml_off as i32);
            w(&mut b, 24, vars.len() as i32);
            w(&mut b, 28, var_hdr_off as i32);
            w(&mut b, 32, 1);
            w(&mut b, 36, buf_len as i32);
            w(&mut b, 48, 1);
            w(&mut b, 52, buf_off as i32);
            b[var_hdr_off..yaml_off].copy_from_slice(&hdrs);
            b[yaml_off..yaml_off + yaml.len()].copy_from_slice(yaml.as_bytes());
            Self {
                bytes: b,
                offsets,
                buf_offset: buf_off,
            }
        }

        fn slot(&mut self, name: &str, idx: usize) -> &mut [u8] {
            let (o, size) = self.offsets[name];
            let start = self.buf_offset + o + idx * size;
            &mut self.bytes[start..start + size]
        }

        pub fn set_f32(&mut self, name: &str, idx: usize, v: f32) {
            self.slot(name, idx).copy_from_slice(&v.to_le_bytes());
        }
        pub fn set_f64(&mut self, name: &str, v: f64) {
            self.slot(name, 0).copy_from_slice(&v.to_le_bytes());
        }
        pub fn set_i32(&mut self, name: &str, idx: usize, v: i32) {
            self.slot(name, idx).copy_from_slice(&v.to_le_bytes());
        }
        pub fn set_bool(&mut self, name: &str, idx: usize, v: bool) {
            self.slot(name, idx)[0] = v as u8;
        }
        pub fn bump_tick(&mut self) {
            let t = i32::from_le_bytes(self.bytes[48..52].try_into().unwrap()) + 1;
            self.bytes[48..52].copy_from_slice(&t.to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    #[test]
    fn picks_latest_of_rotating_buffers() {
        let mut b = vec![0u8; HEADER_LEN];
        let w = |b: &mut Vec<u8>, o: usize, v: i32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        w(&mut b, 32, 3); // num_buf
        for (i, (tick, off)) in [(10, 1000), (12, 2000), (11, 3000), (99, 4000)]
            .iter()
            .enumerate()
        {
            w(&mut b, 48 + 16 * i, *tick);
            w(&mut b, 52 + 16 * i, *off);
        }
        let h = Header::parse(&b).unwrap();
        assert_eq!(
            h.bufs[1],
            VarBuf {
                tick_count: 12,
                offset: 2000
            }
        );
        assert_eq!(
            h.bufs[2],
            VarBuf {
                tick_count: 11,
                offset: 3000
            }
        );
        // Buffer 3 is beyond num_buf and ignored.
        assert_eq!(
            h.latest_buf(),
            Some((
                1,
                VarBuf {
                    tick_count: 12,
                    offset: 2000
                }
            ))
        );
        assert!(!h.connected());
        assert!(Header::parse(&b[..HEADER_LEN - 1]).is_none());
    }

    #[test]
    fn typed_access_across_var_types() {
        let mut img = Image::new(
            &[
                Var {
                    name: "C",
                    ty: 0,
                    count: 1,
                },
                Var {
                    name: "B",
                    ty: 1,
                    count: 2,
                },
                Var {
                    name: "Bits",
                    ty: 3,
                    count: 1,
                },
                Var {
                    name: "F",
                    ty: 4,
                    count: 2,
                },
                Var {
                    name: "Bad",
                    ty: 2,
                    count: 1,
                },
            ],
            "",
        );
        // Corrupt the 5th variable header's type to an unknown value.
        let bad = HEADER_LEN + 4 * VAR_HEADER_LEN;
        img.bytes[bad..bad + 4].copy_from_slice(&9i32.to_le_bytes());
        let (o, _) = img.offsets["C"];
        img.bytes[img.buf_offset + o] = 7;
        img.set_bool("B", 1, true);
        img.set_i32("Bits", 0, -2);
        img.set_f32("F", 1, 3.75);
        let h = Header::parse(&img.bytes).unwrap();
        let vt = VarTable::parse(
            &img.bytes[h.var_header_offset as usize..],
            h.num_vars as usize,
        );
        assert_eq!(vt.len(), 4, "unknown type skipped");
        assert!(!vt.is_empty());
        assert!(VarTable::default().is_empty());
        assert_eq!(vt.get("F").unwrap().ty, VarType::Float);
        let data = &img.bytes[img.buf_offset..];
        let v = vt.view(data);
        assert_eq!(v.f64("C"), Some(7.0));
        assert_eq!(v.i32("C"), Some(7));
        assert_eq!(v.bool_at("B", 0), Some(false));
        assert_eq!(v.bool_at("B", 1), Some(true));
        assert_eq!(v.bool("B"), Some(false));
        assert_eq!(v.bool("Missing"), None);
        assert_eq!(v.i32("Bits"), Some(-2));
        assert_eq!(v.u32("Bits"), Some(u32::MAX - 1));
        assert_eq!(v.f64("Bits"), Some(-2.0));
        assert_eq!(v.i32_at("F", 1), Some(3), "floats truncate");
        assert_eq!(v.f64_at("F", 1), Some(3.75));
        assert_eq!(v.f64_at("F", 2), None);
        assert_eq!(v.i32_at("Bits", 1), None);
        // Truncated data buffer.
        let short = vt.view(&data[..1]);
        assert_eq!(short.f64("F"), None);
        assert_eq!(short.i32("Bits"), None);
    }

    #[test]
    fn parses_header_and_vars() {
        let mut img = Image::new(
            &[
                Var {
                    name: "SessionTime",
                    ty: 5,
                    count: 1,
                },
                Var {
                    name: "CarIdxLap",
                    ty: 2,
                    count: 64,
                },
                Var {
                    name: "Speed",
                    ty: 4,
                    count: 1,
                },
                Var {
                    name: "OnPitRoad",
                    ty: 1,
                    count: 1,
                },
            ],
            "WeekendInfo:\n",
        );
        img.set_f64("SessionTime", 12.5);
        img.set_i32("CarIdxLap", 5, 7);
        img.set_f32("Speed", 0, 42.0);
        img.set_bool("OnPitRoad", 0, true);

        let h = Header::parse(&img.bytes).unwrap();
        assert!(h.connected());
        assert_eq!(h.num_vars, 4);
        let vt = VarTable::parse(
            &img.bytes[h.var_header_offset as usize..],
            h.num_vars as usize,
        );
        assert_eq!(vt.len(), 4);
        let (_, buf) = h.latest_buf().unwrap();
        let data = &img.bytes[buf.offset as usize..buf.offset as usize + h.buf_len as usize];
        let v = vt.view(data);
        assert_eq!(v.f64("SessionTime"), Some(12.5));
        assert_eq!(v.i32_at("CarIdxLap", 5), Some(7));
        assert_eq!(v.i32_at("CarIdxLap", 64), None);
        assert_eq!(v.f64("Speed"), Some(42.0));
        assert_eq!(v.bool("OnPitRoad"), Some(true));
        assert_eq!(v.f64("Missing"), None);
    }
}
