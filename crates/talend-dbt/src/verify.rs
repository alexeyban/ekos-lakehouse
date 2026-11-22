//! EKOS RFC 0268 phase 3: the pure half of the differential run. How each landed file is read
//! (from the job's own file component), parsing a file that way, and comparing a model's rows with
//! the table the Talend jobs wrote. Executing the models is the caller's (a throwaway Trino).

use std::collections::{BTreeMap, BTreeSet};

use ekos_lakehouse_etl_expr::KType;
use ekos_lakehouse_etl_rel::{NoCatalog, Op, SourceQuery};
use ekos_lakehouse_talend_rel::{Options, lower_job_with};
use ekos_talend::Project;
use serde::{Deserialize, Serialize};

/// How one `talend_files` source is read: the file component's own settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FileSpec {
    /// The source's table name: the file as written, or a `tFileList`'s masks (`a*.csv|b*.csv`).
    pub file: String,
    pub job: String,
    pub component: String,
    /// Columns in file order, with their types; `$path` last when the file comes from a list.
    pub columns: Vec<(String, KType)>,
    pub separator: String,
    pub header: usize,
    pub encoding: String,
    pub remove_empty_rows: bool,
    /// `CSV_OPTION`: quoted fields. Not supported yet: such a source is not landed.
    pub csv_quoting: bool,
    /// A workbook (`tFileInputExcel`): its sheets, read cell by cell.
    #[serde(default)]
    pub excel: bool,
    /// A date column's pattern as the schema declares it (`dd-MM-yyyy`), by column.
    #[serde(default)]
    pub patterns: BTreeMap<String, String>,
}

/// Every file source of the project's latest jobs, with how its component reads it.
pub fn file_specs(project: &Project, options: &Options) -> Vec<FileSpec> {
    let mut out: Vec<FileSpec> = Vec::new();
    for j in project.jobs.iter().filter(|j| j.latest) {
        let job = project.expand(j).job;
        let l = lower_job_with(&job, &project.routines, &NoCatalog, options);
        for n in &l.ir.nodes {
            let Op::Source {
                query: SourceQuery::Sql { tables, .. },
            } = &n.op
            else {
                continue;
            };
            let [t] = tables.as_slice() else { continue };
            if t.schema.as_deref() != Some("files") {
                continue;
            }
            let Some(c) = job.component(&n.step) else {
                continue;
            };
            let p = |name: &str| {
                c.params
                    .get(name)
                    .and_then(ekos_talend::Param::text)
                    .map(|v| job.fold(v))
                    .filter(|f| f.complete())
                    .map(|f| f.text)
            };
            let flag = |name: &str| p(name).is_some_and(|v| v.eq_ignore_ascii_case("true"));
            let spec = FileSpec {
                file: t.table.clone(),
                job: job.name.clone(),
                component: n.step.clone(),
                columns: n.schema.iter().map(|f| (f.name.clone(), f.ty)).collect(),
                separator: p("FIELDSEPARATOR").unwrap_or_else(|| ";".into()),
                header: p("HEADER").and_then(|h| h.trim().parse().ok()).unwrap_or(0),
                encoding: p("ENCODING").unwrap_or_else(|| "UTF-8".into()),
                remove_empty_rows: flag("REMOVE_EMPTY_ROW"),
                csv_quoting: flag("CSV_OPTION"),
                excel: c.component_type == "tFileInputExcel",
                patterns: c
                    .schemas
                    .iter()
                    .flat_map(|s| &s.columns)
                    .filter_map(|col| {
                        let p = col.pattern.as_deref()?.trim().trim_matches('"');
                        (!p.is_empty()).then(|| (col.name.clone(), p.to_string()))
                    })
                    .collect(),
            };
            if !out.iter().any(|s| s.file == spec.file) {
                out.push(spec);
            }
        }
    }
    out
}

/// Whether `name` matches one of `masks` (`|`-separated globs with `*` and `?`), case-sensitive.
pub fn mask_matches(masks: &str, name: &str) -> bool {
    fn glob(p: &[u8], s: &[u8]) -> bool {
        match (p.first(), s.first()) {
            (None, None) => true,
            (Some(b'*'), _) => glob(&p[1..], s) || (!s.is_empty() && glob(p, &s[1..])),
            (Some(b'?'), Some(_)) => glob(&p[1..], &s[1..]),
            (Some(a), Some(b)) if a == b => glob(&p[1..], &s[1..]),
            _ => false,
        }
    }
    masks
        .split('|')
        .any(|m| glob(m.as_bytes(), name.as_bytes()))
}

/// A file's rows as the component reads them: lines end at `\r\n`, `\r` or `\n` (as Talend's
/// delimited reader does), the header lines skipped, empty lines dropped when asked, fields split on
/// the separator with no quoting, missing fields null, extra ones ignored. `path` fills `$path`.
pub fn parse_file(
    bytes: &[u8],
    spec: &FileSpec,
    path: &str,
) -> Result<Vec<Vec<Option<String>>>, String> {
    if spec.excel {
        return parse_xlsx(bytes, spec, path);
    }
    if spec.csv_quoting {
        return Err(format!(
            "{}: quoted CSV is not supported yet",
            spec.component
        ));
    }
    let text = match spec.encoding.to_ascii_uppercase().as_str() {
        "UTF-8" | "UTF8" => String::from_utf8(bytes.to_vec()).map_err(|e| e.to_string())?,
        "ISO-8859-1" | "ISO-8859-15" | "LATIN1" | "WINDOWS-1252" | "CP1252" => {
            bytes.iter().map(|b| *b as char).collect()
        }
        other => {
            return Err(format!(
                "{}: encoding {other} is not supported yet",
                spec.component
            ));
        }
    };
    let text = text.strip_prefix('\u{feff}').unwrap_or(&text);
    let data: Vec<&(String, KType)> = spec.columns.iter().filter(|(n, _)| n != "$path").collect();
    let with_path = spec.columns.iter().any(|(n, _)| n == "$path");
    let mut rows = Vec::new();
    for line in text
        .split("\r\n")
        .flat_map(|l| l.split(['\r', '\n']))
        .skip(spec.header)
    {
        if line.is_empty() && spec.remove_empty_rows {
            continue;
        }
        let fields: Vec<&str> = line.split(spec.separator.as_str()).collect();
        let mut row: Vec<Option<String>> = Vec::new();
        for (i, (name, ty)) in data.iter().enumerate() {
            let v = fields.get(i).copied();
            // An empty field is null except for text, which Talend keeps as "".
            let v = v.filter(|v| !v.is_empty() || *ty == KType::String);
            row.push(match (v, spec.patterns.get(name)) {
                (Some(v), Some(p)) if matches!(ty, KType::Date | KType::Timestamp) => Some(
                    java_date(v, p).map_err(|e| format!("{}: `{name}`: {e}", spec.component))?,
                ),
                (v, _) => v.map(str::to_string),
            });
        }
        if with_path {
            row.push(Some(path.to_string()));
        }
        rows.push(row);
    }
    // A last line ended by the row separator is not a row.
    if text.ends_with(['\r', '\n'])
        && rows.last().is_some_and(|r| {
            r.iter()
                .take(data.len())
                .all(|v| v.as_deref().unwrap_or("").is_empty())
        })
    {
        rows.pop();
    }
    Ok(rows)
}

/// A workbook's rows as `tFileInputExcel` reads them in user mode: every sheet in order (the
/// component's `ALL_SHEETS`), the header rows skipped once, each row's cells from the first column
/// by position. A cell is its shared string or its number; a number in a date column is Excel's
/// day count from 1899-12-30, given as `yyyy-MM-dd 00:00:00`.
pub fn parse_xlsx(
    bytes: &[u8],
    spec: &FileSpec,
    path: &str,
) -> Result<Vec<Vec<Option<String>>>, String> {
    use std::io::Read;
    let mut z = zip::ZipArchive::new(std::io::Cursor::new(bytes)).map_err(|e| e.to_string())?;
    let mut read = |name: &str| -> Option<String> {
        let mut f = z.by_name(name).ok()?;
        let mut s = String::new();
        f.read_to_string(&mut s).ok()?;
        Some(s)
    };
    let shared: Vec<String> = match read("xl/sharedStrings.xml") {
        Some(x) => {
            let d = roxmltree::Document::parse(&x).map_err(|e| e.to_string())?;
            d.descendants()
                .filter(|n| n.has_tag_name("si"))
                .map(|si| {
                    si.descendants()
                        .filter(|n| n.has_tag_name("t"))
                        .filter_map(|t| t.text())
                        .collect()
                })
                .collect()
        }
        None => Vec::new(),
    };
    let mut sheets: Vec<(usize, String)> = Vec::new();
    for i in 1..=64 {
        match read(&format!("xl/worksheets/sheet{i}.xml")) {
            Some(x) => sheets.push((i, x)),
            None => break,
        }
    }
    let data: Vec<&(String, KType)> = spec.columns.iter().filter(|(n, _)| n != "$path").collect();
    let with_path = spec.columns.iter().any(|(n, _)| n == "$path");
    let col_index = |r: &str| -> usize {
        r.chars()
            .take_while(char::is_ascii_alphabetic)
            .fold(0usize, |a, c| {
                a * 26 + (c.to_ascii_uppercase() as usize - 'A' as usize + 1)
            })
            .saturating_sub(1)
    };
    let mut rows = Vec::new();
    let mut skipped = 0usize;
    for (_, x) in &sheets {
        let d = roxmltree::Document::parse(x).map_err(|e| e.to_string())?;
        for r in d.descendants().filter(|n| n.has_tag_name("row")) {
            if skipped < spec.header {
                skipped += 1;
                continue;
            }
            let mut cells: Vec<Option<String>> = vec![None; data.len()];
            for c in r.children().filter(|n| n.has_tag_name("c")) {
                let i = col_index(c.attribute("r").unwrap_or("A"));
                let Some((_, ty)) = data.get(i) else { continue };
                let raw = c
                    .children()
                    .find(|n| n.has_tag_name("v"))
                    .and_then(|v| v.text());
                let value = match (c.attribute("t"), raw) {
                    (Some("s"), Some(v)) => {
                        v.parse::<usize>().ok().and_then(|k| shared.get(k).cloned())
                    }
                    (Some("inlineStr"), _) => Some(
                        c.descendants()
                            .filter(|n| n.has_tag_name("t"))
                            .filter_map(|t| t.text())
                            .collect(),
                    ),
                    (_, Some(v)) if matches!(ty, KType::Date | KType::Timestamp) => {
                        let days = v.parse::<f64>().map_err(|e| e.to_string())? as i64;
                        Some(format!("{} 00:00:00", civil_from_days(days - 25_569)))
                    }
                    (_, v) => v.map(str::to_string),
                };
                cells[i] = value;
            }
            if spec.remove_empty_rows && cells.iter().all(Option::is_none) {
                continue;
            }
            if with_path {
                cells.push(Some(path.to_string()));
            }
            rows.push(cells);
        }
    }
    Ok(rows)
}

/// A date written in a numeric Java `SimpleDateFormat` pattern (`dd-MM-yyyy`, `yyyyMMdd HH:mm`), as
/// `yyyy-MM-dd HH:mm:ss`. A pattern with names (`MMM`, `EEE`) or zones is refused.
pub fn java_date(v: &str, pattern: &str) -> Result<String, String> {
    let (mut y, mut mo, mut d, mut h, mut mi, mut s) = (0i64, 1i64, 1i64, 0i64, 0i64, 0i64);
    let (p, b) = (pattern.as_bytes(), v.as_bytes());
    let (mut i, mut j) = (0usize, 0usize);
    while i < p.len() {
        let c = p[i];
        let run = p[i..].iter().take_while(|x| **x == c).count();
        if c.is_ascii_alphabetic() {
            if !matches!(c, b'y' | b'M' | b'd' | b'H' | b'm' | b's') || (c == b'M' && run > 2) {
                return Err(format!("pattern `{pattern}` is not numeric"));
            }
            // A field takes its width, or more digits when the pattern's next character is not a
            // digit field (Java's lenient numbers).
            let next_is_field = p.get(i + run).is_some_and(u8::is_ascii_alphabetic);
            let len = if next_is_field {
                run.max(if c == b'y' { 4 } else { 2 })
                    .min(b.len().saturating_sub(j))
            } else {
                b[j..].iter().take_while(|x| x.is_ascii_digit()).count()
            };
            let n: i64 = std::str::from_utf8(&b[j..j + len])
                .ok()
                .and_then(|t| t.parse().ok())
                .ok_or_else(|| format!("`{v}` does not match `{pattern}`"))?;
            match c {
                b'y' => y = n,
                b'M' => mo = n,
                b'd' => d = n,
                b'H' => h = n,
                b'm' => mi = n,
                _ => s = n,
            }
            j += len;
            i += run;
        } else {
            if b.get(j) != Some(&c) {
                return Err(format!("`{v}` does not match `{pattern}`"));
            }
            i += 1;
            j += 1;
        }
    }
    if j != b.len() {
        return Err(format!("`{v}` does not match `{pattern}`"));
    }
    Ok(format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02}"))
}

/// `yyyy-MM-dd` of a day count since 1970-01-01 (Howard Hinnant's algorithm).
fn civil_from_days(z: i64) -> String {
    let z = z + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{:02}-{:02}", if m <= 2 { y + 1 } else { y }, m, d)
}

/// A table PostgreSQL wrote with `\copy … with (format csv, header)`: its header and rows. A
/// quoted field is text (`""` is an empty string); an unquoted empty field is NULL.
pub fn parse_pg_csv(text: &str) -> (Vec<String>, Vec<Vec<Option<String>>>) {
    let mut rows: Vec<Vec<Option<String>>> = Vec::new();
    let mut row: Vec<Option<String>> = Vec::new();
    let mut field = String::new();
    let (mut quoted, mut in_quotes, mut any) = (false, false, false);
    let mut chars = text.chars().peekable();
    let end_field = |row: &mut Vec<Option<String>>, field: &mut String, quoted: &mut bool| {
        row.push(if field.is_empty() && !*quoted {
            None
        } else {
            Some(std::mem::take(field))
        });
        *quoted = false;
    };
    while let Some(c) = chars.next() {
        any = true;
        if in_quotes {
            match c {
                '"' if chars.peek() == Some(&'"') => {
                    chars.next();
                    field.push('"');
                }
                '"' => in_quotes = false,
                _ => field.push(c),
            }
            continue;
        }
        match c {
            '"' => {
                in_quotes = true;
                quoted = true;
            }
            ',' => end_field(&mut row, &mut field, &mut quoted),
            '\r' if chars.peek() == Some(&'\n') => {}
            '\n' | '\r' => {
                end_field(&mut row, &mut field, &mut quoted);
                rows.push(std::mem::take(&mut row));
                any = false;
            }
            _ => field.push(c),
        }
    }
    if any {
        end_field(&mut row, &mut field, &mut quoted);
        rows.push(row);
    }
    let header = rows
        .first()
        .map(|h| h.iter().map(|v| v.clone().unwrap_or_default()).collect())
        .unwrap_or_default();
    (header, rows.into_iter().skip(1).collect())
}

/// A value as text both sides can agree on: trimmed; booleans `true`/`false`; a number without
/// trailing fraction zeros; a timestamp without a zero fraction. `None` is SQL null.
pub fn normalize(v: Option<&str>) -> Option<String> {
    let v = v?.trim();
    match v {
        "t" | "true" | "TRUE" => return Some("true".into()),
        "f" | "false" | "FALSE" => return Some("false".into()),
        _ => {}
    }
    let numeric = !v.is_empty()
        && v.chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '-' | '+' | '.'))
        && v.chars().filter(|c| *c == '.').count() <= 1
        && v.chars().any(|c| c.is_ascii_digit());
    if numeric && v.contains('.') {
        let t = v.trim_end_matches('0').trim_end_matches('.');
        return Some(if t.is_empty() || t == "-" {
            "0".into()
        } else {
            t.to_string()
        });
    }
    // A date and the timestamp of its midnight are one value: `date` in one engine, `timestamp`
    // in the other.
    if v.len() >= 19 && v.as_bytes()[4] == b'-' && v.as_bytes()[10] == b' ' {
        if v[11..].chars().all(|c| matches!(c, '0' | ':' | '.')) {
            return Some(v[..10].to_string());
        }
        // `2026-11-22 16:16:30.000` → `2026-11-22 16:16:30`; `…30.120` → `…30.12`.
        if let Some((head, frac)) = v.split_once('.') {
            let frac = frac.trim_end_matches('0');
            return Some(if frac.is_empty() {
                head.to_string()
            } else {
                format!("{head}.{frac}")
            });
        }
    }
    Some(v.to_string())
}

/// The outcome of one model's comparison.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "result")]
pub enum Outcome {
    Equal {
        rows: usize,
    },
    Differs {
        talend_rows: usize,
        dbt_rows: usize,
        /// Up to five rows on one side only, each as its values.
        only_talend: Vec<Vec<Option<String>>>,
        only_dbt: Vec<Vec<Option<String>>>,
        only_talend_count: usize,
        only_dbt_count: usize,
    },
    NotVerified {
        reason: String,
    },
}

/// One model compared with the table the jobs wrote.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelCheck {
    pub model: String,
    pub table: String,
    /// The columns compared.
    pub columns: Vec<String>,
    /// Columns left out, each with why.
    pub excluded: Vec<(String, String)>,
    #[serde(flatten)]
    pub outcome: Outcome,
    /// What the model's jobs wrote to their error output in the sandbox (a refused statement,
    /// rolled back while the job still exited 0): the reason a table may differ.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub job_errors: Vec<String>,
}

/// Compares two tables over `columns` (names as the Talend side spells them): multisets of
/// normalized rows. Each side is `(header, rows)`; columns are matched case-insensitively.
pub fn compare(
    columns: &[String],
    talend: (&[String], &[Vec<Option<String>>]),
    dbt: (&[String], &[Vec<Option<String>>]),
) -> Result<Outcome, String> {
    let pick = |header: &[String], side: &str| -> Result<Vec<usize>, String> {
        columns
            .iter()
            .map(|c| {
                header
                    .iter()
                    .position(|h| h.eq_ignore_ascii_case(c))
                    .ok_or_else(|| format!("the {side} side has no column `{c}`"))
            })
            .collect()
    };
    let (ti, di) = (pick(talend.0, "Talend")?, pick(dbt.0, "dbt")?);
    let bag = |rows: &[Vec<Option<String>>], idx: &[usize]| {
        let mut m: BTreeMap<Vec<Option<String>>, usize> = BTreeMap::new();
        for r in rows {
            let k: Vec<Option<String>> = idx
                .iter()
                .map(|i| normalize(r.get(*i).and_then(|v| v.as_deref())))
                .collect();
            *m.entry(k).or_default() += 1;
        }
        m
    };
    let (t, d) = (bag(talend.1, &ti), bag(dbt.1, &di));
    let minus = |a: &BTreeMap<Vec<Option<String>>, usize>,
                 b: &BTreeMap<Vec<Option<String>>, usize>| {
        let mut out = Vec::new();
        let mut n = 0;
        for (k, c) in a {
            let extra = c.saturating_sub(*b.get(k).unwrap_or(&0));
            n += extra;
            for _ in 0..extra.min(5usize.saturating_sub(out.len())) {
                out.push(k.clone());
            }
        }
        (out, n)
    };
    let (only_talend, only_talend_count) = minus(&t, &d);
    let (only_dbt, only_dbt_count) = minus(&d, &t);
    Ok(if only_talend_count == 0 && only_dbt_count == 0 {
        Outcome::Equal {
            rows: talend.1.len(),
        }
    } else {
        Outcome::Differs {
            talend_rows: talend.1.len(),
            dbt_rows: dbt.1.len(),
            only_talend,
            only_dbt,
            only_talend_count,
            only_dbt_count,
        }
    })
}

/// The columns to compare and those left out: the Talend table's columns that the model also has,
/// without its clock columns and without database-assigned keys (`surrogates`, by name, in any
/// table: a fact's dimension key holds the same values).
pub fn columns_to_compare(
    talend: &[String],
    dbt: &[String],
    clock: &[String],
    surrogates: &BTreeSet<String>,
) -> (Vec<String>, Vec<(String, String)>) {
    let mut keep = Vec::new();
    let mut out = Vec::new();
    for c in talend {
        let l = c.to_lowercase();
        if !dbt.iter().any(|d| d.eq_ignore_ascii_case(c)) {
            out.push((c.clone(), "the model does not write it".into()));
        } else if clock.iter().any(|k| k.eq_ignore_ascii_case(c)) {
            out.push((c.clone(), "set from the clock: the run's time".into()));
        } else if surrogates.contains(&l) {
            out.push((
                c.clone(),
                "a key the database assigns (SERIAL); dbt's version id stands for it".into(),
            ));
        } else {
            keep.push(c.clone());
        }
    }
    (keep, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(cols: &[(&str, KType)]) -> FileSpec {
        FileSpec {
            file: "ICOM_*_X.csv".into(),
            job: "j".into(),
            component: "tFileInputDelimited_1".into(),
            columns: cols.iter().map(|(n, t)| (n.to_string(), *t)).collect(),
            separator: "|".into(),
            header: 1,
            encoding: "UTF-8".into(),
            remove_empty_rows: true,
            csv_quoting: false,
            excel: false,
            patterns: BTreeMap::new(),
        }
    }

    #[test]
    fn a_date_is_read_with_its_columns_pattern() {
        assert_eq!(
            java_date("01-03-2023", "dd-MM-yyyy").unwrap(),
            "2023-03-01 00:00:00"
        );
        assert_eq!(
            java_date("20230301 1405", "yyyyMMdd HHmm").unwrap(),
            "2023-03-01 14:05:00"
        );
        assert!(java_date("01-Mar-2023", "dd-MMM-yyyy").is_err());
        assert!(java_date("2023/03/01", "dd-MM-yyyy").is_err());
    }

    #[test]
    fn an_excel_day_number_is_its_date() {
        // 43831 is 2020-01-01 in Excel's 1900 system (fixture B's first holiday).
        assert_eq!(civil_from_days(43_831 - 25_569), "2020-01-01");
        assert_eq!(civil_from_days(46_016 - 25_569), "2025-12-25");
    }

    #[test]
    fn a_file_with_carriage_returns_is_read_as_talend_reads_it() {
        let s = spec(&[
            ("A", KType::String),
            ("N", KType::Integer),
            ("$path", KType::String),
        ]);
        let rows = parse_file(b"A|N\rx|1\r|\r\ry\r", &s, "C:\\d\\f.csv").unwrap();
        assert_eq!(
            rows,
            vec![
                vec![
                    Some("x".into()),
                    Some("1".into()),
                    Some("C:\\d\\f.csv".into())
                ],
                vec![Some("".into()), None, Some("C:\\d\\f.csv".into())],
                vec![Some("y".into()), None, Some("C:\\d\\f.csv".into())],
            ]
        );
    }

    #[test]
    fn a_postgresql_csv_keeps_null_apart_from_empty_text() {
        let (h, rows) = parse_pg_csv("A,B,C\n1,,\"\"\n\"x,\"\"y\"\"\nz\",2,3\n");
        assert_eq!(h, vec!["A", "B", "C"]);
        assert_eq!(rows[0], vec![Some("1".into()), None, Some("".into())]);
        assert_eq!(rows[1][0].as_deref(), Some("x,\"y\"\nz"));
        assert_eq!(rows.len(), 2);
    }

    #[test]
    fn masks_match_like_a_file_list() {
        assert!(mask_matches(
            "ICOM_*_TYPE_CLIENT.csv",
            "ICOM_20250605_TYPE_CLIENT.csv"
        ));
        assert!(!mask_matches(
            "ICOM_*_TYPE_CLIENT.csv",
            "ICOM_20250605_CLIENT.csv"
        ));
        assert!(mask_matches("a*.csv|b?.txt", "b1.txt"));
    }

    #[test]
    fn values_normalize_across_engines() {
        assert_eq!(normalize(Some("12.50")), Some("12.5".into()));
        assert_eq!(normalize(Some("3.000")), Some("3".into()));
        assert_eq!(normalize(Some("t")), Some("true".into()));
        assert_eq!(
            normalize(Some("2026-11-22 16:16:30.000")),
            Some("2026-11-22 16:16:30".into())
        );
        assert_eq!(
            normalize(Some("2020-01-01 00:00:00")),
            Some("2020-01-01".into())
        );
        assert_eq!(
            normalize(Some("2020-01-01 00:00:00.000")),
            Some("2020-01-01".into())
        );
        assert_eq!(normalize(Some("T001")), Some("T001".into()));
        assert_eq!(normalize(None), None);
    }

    #[test]
    fn rows_compare_as_multisets_and_a_missing_row_is_found() {
        let h: Vec<String> = vec!["A".into(), "B".into()];
        let hd: Vec<String> = vec!["b".into(), "a".into()];
        let t = vec![
            vec![Some("1".into()), Some("x".into())],
            vec![Some("1".into()), Some("x".into())],
        ];
        let d = vec![
            vec![Some("x".into()), Some("1.0".into())],
            vec![Some("x".into()), Some("1".into())],
        ];
        let cols = vec!["A".to_string(), "B".to_string()];
        assert_eq!(
            compare(&cols, (&h, &t), (&hd, &d)).unwrap(),
            Outcome::Equal { rows: 2 }
        );
        let Outcome::Differs {
            only_talend_count,
            only_dbt_count,
            ..
        } = compare(&cols, (&h, &t), (&hd, &d[..1])).unwrap()
        else {
            panic!()
        };
        assert_eq!((only_talend_count, only_dbt_count), (1, 0));
    }

    #[test]
    fn clock_columns_and_database_keys_are_left_out_with_why() {
        let t: Vec<String> = ["ID_DIM", "CD", "DT_INSERTION", "EXTRA"]
            .map(String::from)
            .to_vec();
        let d: Vec<String> = ["id_dim", "cd", "dt_insertion"].map(String::from).to_vec();
        let (keep, out) = columns_to_compare(
            &t,
            &d,
            &["DT_INSERTION".into()],
            &["id_dim".to_string()].into(),
        );
        assert_eq!(keep, vec!["CD"]);
        assert_eq!(out.len(), 3);
    }
}
