//! Git fast-export stream parser and fast-import stream writer.
//!
//! `git fast-export` emits each commit's file operations relative to its *first parent*,
//! which is exactly Sapwood's changeset model, so import is a single streaming pass with
//! no tree diffing.

use std::io::{BufRead, Write};

use anyhow::{Context, Result, bail};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DataRef {
    Mark(u64),
    Sha(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Op {
    Modify { mode: u32, data: DataRef, path: String },
    Delete(String),
    DeleteAll,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GitCommit {
    pub refname: String,
    pub mark: Option<u64>,
    pub original_oid: Option<String>,
    /// `Name <email>`
    pub author: String,
    pub timestamp_s: i64,
    pub message: String,
    pub from: Option<u64>,
    pub merges: Vec<u64>,
    pub ops: Vec<Op>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Blob { mark: u64, data: Vec<u8> },
    Commit(GitCommit),
}

pub struct Parser<R> {
    r: R,
    peeked: Option<String>,
}

fn parse_mark(s: &str) -> Result<u64> {
    s.strip_prefix(':').context("expected :mark")?.parse().context("bad mark number")
}

/// Undo git's C-style path quoting (`"a\"b\303\251"`).
pub fn unquote(s: &str) -> Result<String> {
    let Some(inner) = s.strip_prefix('"').and_then(|s| s.strip_suffix('"')) else {
        return Ok(s.to_owned());
    };
    let mut out = Vec::with_capacity(inner.len());
    let mut it = inner.bytes().peekable();
    while let Some(b) = it.next() {
        if b != b'\\' {
            out.push(b);
            continue;
        }
        let e = it.next().context("dangling escape")?;
        out.push(match e {
            b'n' => b'\n',
            b't' => b'\t',
            b'r' => b'\r',
            b'a' => 7,
            b'b' => 8,
            b'f' => 12,
            b'v' => 11,
            b'0'..=b'7' => {
                let mut v = (e - b'0') as u32;
                for _ in 0..2 {
                    match it.peek() {
                        Some(d @ b'0'..=b'7') => {
                            v = v * 8 + (d - b'0') as u32;
                            it.next();
                        }
                        _ => break,
                    }
                }
                u8::try_from(v).context("octal escape out of range")?
            }
            other => other,
        });
    }
    String::from_utf8(out).context("path is not UTF-8")
}

/// Quote a path for fast-import when it needs it.
pub fn quote(path: &str) -> String {
    if !path.bytes().any(|b| b == b'"' || b == b'\\' || b == b'\n' || b < 0x20 || path.starts_with(' ')) {
        return path.to_owned();
    }
    let mut out = String::from("\"");
    for c in path.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\{:03o}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// `Name <email> 1700000000 +0000` -> (`Name <email>`, seconds).
fn parse_ident(s: &str) -> Result<(String, i64)> {
    let mut parts = s.rsplitn(3, ' ');
    let _tz = parts.next().context("ident tz")?;
    let ts = parts.next().context("ident time")?.parse().context("ident time")?;
    Ok((parts.next().context("ident name")?.to_owned(), ts))
}

impl<R: BufRead> Parser<R> {
    pub fn new(r: R) -> Self {
        Self { r, peeked: None }
    }

    fn line(&mut self) -> Result<Option<String>> {
        if let Some(l) = self.peeked.take() {
            return Ok(Some(l));
        }
        let mut buf = Vec::new();
        if self.r.read_until(b'\n', &mut buf)? == 0 {
            return Ok(None);
        }
        if buf.last() == Some(&b'\n') {
            buf.pop();
        }
        match String::from_utf8(buf) {
            Ok(s) => Ok(Some(s)),
            // Paths must round-trip exactly; refuse rather than silently rename a file.
            Err(e) if e.as_bytes().starts_with(b"M ") || e.as_bytes().starts_with(b"D ") => {
                bail!("non-UTF-8 path in git history: {}", String::from_utf8_lossy(e.as_bytes()))
            }
            // Identity lines from old commits may be Latin-1 even with --reencode.
            Err(e) => Ok(Some(String::from_utf8_lossy(e.as_bytes()).into_owned())),
        }
    }

    fn data(&mut self, header: &str) -> Result<Vec<u8>> {
        let n: usize = header.strip_prefix("data ").context("expected data")?.parse().context("bad data length")?;
        let mut buf = vec![0; n];
        self.r.read_exact(&mut buf)?;
        Ok(buf)
    }

    fn expect_data(&mut self) -> Result<Vec<u8>> {
        loop {
            let l = self.line()?.context("eof before data")?;
            if l.starts_with("data ") {
                return self.data(&l);
            }
        }
    }

    pub fn next_event(&mut self) -> Result<Option<Event>> {
        loop {
            let Some(line) = self.line()? else { return Ok(None) };
            if line == "blob" {
                let mut mark = None;
                loop {
                    let l = self.line()?.context("eof in blob")?;
                    if let Some(m) = l.strip_prefix("mark ") {
                        mark = Some(parse_mark(m)?);
                    } else if l.starts_with("data ") {
                        let data = self.data(&l)?;
                        return Ok(Some(Event::Blob { mark: mark.context("blob without mark")?, data }));
                    }
                }
            } else if let Some(refname) = line.strip_prefix("commit ") {
                return self.commit(refname.to_owned()).map(|c| Some(Event::Commit(c)));
            } else if line.starts_with("tag ") {
                self.expect_data()?; // annotated tags are not imported
            }
            // reset/from, feature, progress, done, blank lines: nothing to do.
        }
    }

    fn commit(&mut self, refname: String) -> Result<GitCommit> {
        let mut c = GitCommit { refname, ..Default::default() };
        loop {
            let l = self.line()?.context("eof in commit header")?;
            if let Some(m) = l.strip_prefix("mark ") {
                c.mark = Some(parse_mark(m)?);
            } else if let Some(o) = l.strip_prefix("original-oid ") {
                c.original_oid = Some(o.to_owned());
            } else if let Some(a) = l.strip_prefix("author ") {
                (c.author, c.timestamp_s) = parse_ident(a)?;
            } else if let Some(a) = l.strip_prefix("committer ") {
                if c.author.is_empty() {
                    (c.author, c.timestamp_s) = parse_ident(a)?;
                }
            } else if l.starts_with("data ") {
                c.message = String::from_utf8_lossy(&self.data(&l)?).into_owned();
                break;
            }
        }
        loop {
            let Some(l) = self.line()? else { break };
            if l.is_empty() {
                // Optional LF after data, or end of commit.
                if c.from.is_none() && c.merges.is_empty() && c.ops.is_empty() {
                    continue;
                }
                break;
            }
            if let Some(f) = l.strip_prefix("from ") {
                c.from = Some(parse_mark(f)?);
            } else if let Some(m) = l.strip_prefix("merge ") {
                c.merges.push(parse_mark(m)?);
            } else if let Some(rest) = l.strip_prefix("M ") {
                let (mode, rest) = rest.split_once(' ').context("M mode")?;
                let (dref, path) = rest.split_once(' ').context("M dataref")?;
                let data = if dref.starts_with(':') { DataRef::Mark(parse_mark(dref)?) } else { DataRef::Sha(dref.to_owned()) };
                let mode = u32::from_str_radix(mode, 8).context("M mode")?;
                c.ops.push(Op::Modify { mode, data, path: unquote(path)? });
            } else if let Some(p) = l.strip_prefix("D ") {
                c.ops.push(Op::Delete(unquote(p)?));
            } else if l == "deleteall" {
                c.ops.push(Op::DeleteAll);
            } else if l.starts_with("C ") || l.starts_with("R ") || l.starts_with("N ") {
                bail!("unsupported fast-export op `{l}` (do not pass -M/-C to fast-export)");
            } else {
                // Next command without a separating blank line.
                self.peeked = Some(l);
                break;
            }
        }
        Ok(c)
    }
}

/// Minimal fast-import writer for `sap export-git`.
pub struct Writer<W: Write> {
    pub w: W,
}

impl<W: Write> Writer<W> {
    pub fn blob(&mut self, mark: u64, data: &[u8]) -> Result<()> {
        write!(self.w, "blob\nmark :{mark}\ndata {}\n", data.len())?;
        self.w.write_all(data)?;
        self.w.write_all(b"\n")?;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    pub fn commit(
        &mut self,
        refname: &str,
        mark: u64,
        author: &str,
        timestamp_s: i64,
        message: &str,
        from: Option<u64>,
        ops: &[(String, Option<(u32, u64)>)],
    ) -> Result<()> {
        let ident = if author.contains('<') { author.to_owned() } else { format!("{author} <>") };
        write!(self.w, "commit {refname}\nmark :{mark}\n")?;
        write!(self.w, "author {ident} {timestamp_s} +0000\ncommitter {ident} {timestamp_s} +0000\n")?;
        write!(self.w, "data {}\n{message}\n", message.len())?;
        if let Some(f) = from {
            writeln!(self.w, "from :{f}")?;
        }
        for (path, op) in ops {
            match op {
                Some((mode, blob)) => writeln!(self.w, "M {mode:o} :{blob} {}", quote(path))?,
                None => writeln!(self.w, "D {}", quote(path))?,
            }
        }
        writeln!(self.w)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "blob\nmark :1\ndata 2\na\n\nreset refs/heads/main\ncommit refs/heads/main\nmark :2\n\
author t <t@t> 1792016068 +0530\ncommitter t <t@t> 1792016068 +0530\ndata 4\none\nM 100644 :1 a\n\n\
commit refs/heads/main\nmark :7\nauthor t <t@t> 5 +0530\ncommitter t <t@t> 5 +0530\ndata 9\ntwo\nline\n\
from :2\nmerge :2\nM 100755 :1 d/x\nM 100644 :1 \"sp ace\\\"q\"\nD a\ndeleteall\n\n\
tag v1\nfrom :7\ntagger t <t@t> 1 +0000\ndata 3\nmsg\ncommit refs/heads/main\nmark :8\n\
author t <t@t> 6 +0000\ncommitter t <t@t> 6 +0000\ndata 0\nfrom :7\nM 160000 0123abcd sub\n";

    #[test]
    fn parses_real_fast_export_shape() {
        let mut p = Parser::new(SAMPLE.as_bytes());
        assert_eq!(p.next_event().unwrap(), Some(Event::Blob { mark: 1, data: b"a\n".to_vec() }));
        let Some(Event::Commit(c)) = p.next_event().unwrap() else { panic!() };
        assert_eq!((c.mark, c.author.as_str(), c.timestamp_s, c.message.as_str()), (Some(2), "t <t@t>", 1792016068, "one\n"));
        assert_eq!(c.ops, vec![Op::Modify { mode: 0o100644, data: DataRef::Mark(1), path: "a".into() }]);
        let Some(Event::Commit(c)) = p.next_event().unwrap() else { panic!() };
        assert_eq!((c.from, c.merges.clone(), c.message.as_str()), (Some(2), vec![2], "two\nline\n"));
        assert_eq!(c.ops[1], Op::Modify { mode: 0o100644, data: DataRef::Mark(1), path: "sp ace\"q".into() });
        assert_eq!(c.ops[2..], [Op::Delete("a".into()), Op::DeleteAll]);
        let Some(Event::Commit(c)) = p.next_event().unwrap() else { panic!() };
        assert_eq!(c.ops, vec![Op::Modify { mode: 0o160000, data: DataRef::Sha("0123abcd".into()), path: "sub".into() }]);
        assert_eq!(p.next_event().unwrap(), None);
    }

    #[test]
    fn latin1_identity_is_tolerated_but_paths_are_strict() {
        let mut ok = b"commit refs/heads/main\nmark :1\nauthor Ren\xe9 <r@x> 1 +0000\ncommitter c <c@x> 1 +0000\ndata 1\nx\n\n".to_vec();
        let Some(Event::Commit(c)) = Parser::new(&ok[..]).next_event().unwrap() else { panic!() };
        assert_eq!(c.author, "Ren\u{fffd} <r@x>");
        ok.splice(ok.len() - 1..ok.len() - 1, b"M 100644 :1 caf\xe9\n".iter().copied());
        assert!(Parser::new(&ok[..]).next_event().unwrap_err().to_string().contains("non-UTF-8 path"));
    }

    #[test]
    fn quoting_roundtrips() {
        for p in ["plain/path.rs", "sp ace", "q\"uote", "back\\slash", "tab\there", "new\nline", "caf\u{e9}"] {
            assert_eq!(unquote(&quote(p)).unwrap(), p, "{p:?}");
        }
        assert_eq!(unquote("\"caf\\303\\251\"").unwrap(), "caf\u{e9}");
    }
}
