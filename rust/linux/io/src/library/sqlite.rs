//! A minimal read-only SQLite reader for one rowid table: enough for Lutris' `pga.db`
//! without linking SQLite. It reads the file's bytes (UTF-8 databases, no WAL), never
//! writes, and answers an error, never a panic, for a file that is not what it expects.

use anyhow::{Result, bail, ensure};
use std::collections::HashMap;

/// One column value of a row.
#[derive(Clone, Debug, PartialEq)]
pub enum Value {
    Null,
    Int(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

impl Value {
    /// The text, when the value is text.
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text(text) => Some(text),
            _ => None,
        }
    }
    /// The integer, when the value is one (a text that reads as a number counts, as SQLite's
    /// column affinity would have stored it).
    pub fn int(&self) -> Option<i64> {
        match self {
            Self::Int(number) => Some(*number),
            Self::Text(text) => text.trim().parse().ok(),
            _ => None,
        }
    }
}

/// A row by column name.
pub type Row = HashMap<String, Value>;

/// The most pages one table read visits: a damaged file with a loop in its tree ends here.
const MAX_PAGES: usize = 200_000;
/// The deepest b-tree this reader follows (a real one is 5 or so deep).
const MAX_DEPTH: usize = 24;

struct Database<'a> {
    bytes: &'a [u8],
    page_size: usize,
    usable: usize,
}

/// The rows of `table` in `bytes`, a whole database file.
pub fn rows(bytes: &[u8], table: &str) -> Result<Vec<Row>> {
    ensure!(
        bytes.starts_with(b"SQLite format 3\0"),
        "not an SQLite database"
    );
    let page_size = match be16(bytes, 16) {
        Some(1) => 65536,
        Some(size) if size >= 512 && size.is_power_of_two() => usize::from(size),
        _ => bail!("invalid SQLite page size"),
    };
    ensure!(
        bytes.get(56..60) == Some(&[0, 0, 0, 1]),
        "only UTF-8 SQLite databases are read"
    );
    let reserved = usize::from(*bytes.get(20).unwrap_or(&0));
    let database = Database {
        bytes,
        page_size,
        usable: page_size - reserved.min(page_size / 2),
    };
    let mut visited = 0;
    let mut master = vec![];
    database.walk(1, 0, &mut visited, &mut |_, record| master.push(record))?;
    // sqlite_master: type, name, tbl_name, rootpage, sql.
    let entry = master.iter().find(|record| {
        record.first().and_then(Value::text) == Some("table")
            && record.get(1).and_then(Value::text) == Some(table)
    });
    let Some(entry) = entry else {
        bail!("no table {table}");
    };
    let root = entry
        .get(3)
        .and_then(Value::int)
        .and_then(|page| usize::try_from(page).ok())
        .filter(|page| *page >= 1);
    let (Some(root), Some(sql)) = (root, entry.get(4).and_then(Value::text)) else {
        bail!("table {table} has no definition");
    };
    let columns = columns(sql);
    ensure!(!columns.is_empty(), "table {table} has no columns");
    let mut result = vec![];
    let mut visited = 0;
    database.walk(root, 0, &mut visited, &mut |rowid, record| {
        let mut row = Row::new();
        for (index, (name, rowid_alias)) in columns.iter().enumerate() {
            let value = match record.get(index) {
                Some(Value::Null) | None if *rowid_alias => Value::Int(rowid),
                Some(value) => value.clone(),
                None => Value::Null,
            };
            row.insert(name.clone(), value);
        }
        result.push(row);
    })?;
    Ok(result)
}

/// The column names of a `CREATE TABLE` statement, each with whether it is the rowid alias
/// (`INTEGER PRIMARY KEY`, which a record stores as NULL).
fn columns(sql: &str) -> Vec<(String, bool)> {
    let (Some(start), Some(end)) = (sql.find('('), sql.rfind(')')) else {
        return vec![];
    };
    if end <= start {
        return vec![];
    }
    let (mut parts, mut depth, mut current) = (vec![], 0usize, String::new());
    for character in sql[start + 1..end].chars() {
        match character {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(std::mem::take(&mut current));
                continue;
            }
            _ => {}
        }
        current.push(character);
    }
    parts.push(current);
    parts
        .iter()
        .filter_map(|part| {
            let part = part.trim();
            let name = part.split_whitespace().next()?;
            let upper = part.to_ascii_uppercase();
            if ["PRIMARY", "UNIQUE", "FOREIGN", "CHECK", "CONSTRAINT"]
                .iter()
                .any(|keyword| upper.starts_with(keyword))
            {
                return None;
            }
            let name = name.trim_matches(['"', '\'', '`', '[', ']']).to_owned();
            let alias = upper.contains("INTEGER PRIMARY KEY");
            Some((name, alias))
        })
        .collect()
}

fn be16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_be_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}
fn be32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_be_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

/// A variable-length integer at `at`: its value and length.
fn varint(bytes: &[u8], at: usize) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for index in 0..9 {
        let byte = *bytes.get(at + index)?;
        if index == 8 {
            return Some(((value << 8) | u64::from(byte), 9));
        }
        value = (value << 7) | u64::from(byte & 0x7f);
        if byte & 0x80 == 0 {
            return Some((value, index + 1));
        }
    }
    None
}

impl Database<'_> {
    fn page(&self, number: usize) -> Result<&[u8]> {
        let start = number
            .checked_sub(1)
            .and_then(|index| index.checked_mul(self.page_size));
        match start.and_then(|start| self.bytes.get(start..start + self.page_size)) {
            Some(page) => Ok(page),
            None => bail!("SQLite page {number} is outside the file"),
        }
    }

    /// Calls `each` with the rowid and decoded record of every row below table page `number`,
    /// in rowid order.
    fn walk(
        &self,
        number: usize,
        depth: usize,
        visited: &mut usize,
        each: &mut dyn FnMut(i64, Vec<Value>),
    ) -> Result<()> {
        *visited += 1;
        ensure!(
            depth <= MAX_DEPTH && *visited <= MAX_PAGES,
            "SQLite table tree is too deep or loops"
        );
        let page = self.page(number)?;
        let base = if number == 1 { 100 } else { 0 };
        let kind = *page.get(base).unwrap_or(&0);
        let count = usize::from(be16(page, base + 3).unwrap_or(0));
        match kind {
            5 => {
                for index in 0..count {
                    let pointer = be16(page, base + 12 + index * 2).map(usize::from);
                    let child = pointer.and_then(|at| be32(page, at));
                    let Some(child) = child else {
                        bail!("damaged SQLite interior page");
                    };
                    self.walk(child as usize, depth + 1, visited, each)?;
                }
                let Some(last) = be32(page, base + 8) else {
                    bail!("damaged SQLite interior page");
                };
                self.walk(last as usize, depth + 1, visited, each)
            }
            13 => {
                for index in 0..count {
                    let Some(at) = be16(page, base + 8 + index * 2).map(usize::from) else {
                        bail!("damaged SQLite leaf page");
                    };
                    let (rowid, record) = self.cell(page, at)?;
                    each(rowid, record);
                }
                Ok(())
            }
            _ => bail!("not a SQLite table page"),
        }
    }

    /// The row of the table leaf cell at `at` in `page`, following its overflow pages.
    fn cell(&self, page: &[u8], at: usize) -> Result<(i64, Vec<Value>)> {
        let damaged = || anyhow::anyhow!("damaged SQLite cell");
        let (size, used) = varint(page, at).ok_or_else(damaged)?;
        let (rowid, used_rowid) = varint(page, at + used).ok_or_else(damaged)?;
        let size = usize::try_from(size).map_err(|_| damaged())?;
        ensure!(size <= self.bytes.len(), "damaged SQLite cell");
        let start = at + used + used_rowid;
        let usable = self.usable;
        let max_local = usable.saturating_sub(35);
        let mut payload;
        if size <= max_local {
            payload = page.get(start..start + size).ok_or_else(damaged)?.to_vec();
        } else {
            let min_local = (usable.saturating_sub(12) * 32 / 255).saturating_sub(23);
            let spill = min_local + (size - min_local) % (usable - 4);
            let local = if spill <= max_local { spill } else { min_local };
            payload = page.get(start..start + local).ok_or_else(damaged)?.to_vec();
            let mut next = be32(page, start + local).ok_or_else(damaged)? as usize;
            let mut hops = 0;
            while payload.len() < size {
                hops += 1;
                ensure!(next != 0 && hops <= MAX_PAGES, "damaged SQLite overflow");
                let overflow = self.page(next)?;
                let take = (size - payload.len()).min(usable - 4);
                payload.extend_from_slice(overflow.get(4..4 + take).ok_or_else(damaged)?);
                next = be32(overflow, 0).ok_or_else(damaged)? as usize;
            }
        }
        Ok((rowid as i64, record(&payload).ok_or_else(damaged)?))
    }
}

/// The values of one record.
fn record(payload: &[u8]) -> Option<Vec<Value>> {
    let (header, used) = varint(payload, 0)?;
    let header = usize::try_from(header).ok()?;
    let mut types = vec![];
    let mut at = used;
    while at < header {
        let (kind, length) = varint(payload, at)?;
        types.push(kind);
        at += length;
    }
    let mut at = header;
    let mut values = vec![];
    for kind in types {
        let length: usize = match kind {
            0 | 8 | 9 | 10 | 11 => 0,
            1 => 1,
            2 => 2,
            3 => 3,
            4 => 4,
            5 => 6,
            6 | 7 => 8,
            kind => usize::try_from((kind - 12) / 2).ok()?,
        };
        let data = payload.get(at..at.checked_add(length)?)?;
        at += length;
        values.push(match kind {
            0 | 10 | 11 => Value::Null,
            8 => Value::Int(0),
            9 => Value::Int(1),
            1..=6 => {
                let mut number = if data[0] & 0x80 != 0 { -1i64 } else { 0 };
                for byte in data {
                    number = (number << 8) | i64::from(*byte);
                }
                Value::Int(number)
            }
            7 => Value::Real(f64::from_be_bytes(data.try_into().ok()?)),
            kind if kind % 2 == 0 => Value::Blob(data.to_vec()),
            _ => Value::Text(String::from_utf8_lossy(data).into_owned()),
        });
    }
    Some(values)
}
