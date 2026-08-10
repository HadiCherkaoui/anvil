// SPDX-FileCopyrightText: Hadi Cherkaoui <contact@hide.cherkaoui.ch>
//
// SPDX-License-Identifier: AGPL-3.0-or-later

//! Pure parsers for the file-browser exec output.
//!
//! Functions are I/O-free and `kube`-free. Tests cover real busybox
//! outputs the helper script emits.

use serde::Serialize;

/// Discriminator for [`FileEntry`].
#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum FileEntryType {
    F,
    D,
    L,
    O,
}

/// One row of a directory listing.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, utoipa::ToSchema)]
pub struct FileEntry {
    pub name: String,
    #[serde(rename = "type")]
    pub entry_type: FileEntryType,
    pub size: u64,
    pub mtime: i64,
}

/// Bulk-read response shape for `GET /api/servers/{id}/files`.
#[derive(Debug, Serialize, utoipa::ToSchema)]
pub struct FileListResponse {
    pub path: String,
    pub entries: Vec<FileEntry>,
}

/// Sentinel emitted by [`LIST_SCRIPT`] when the target is missing or not
/// a directory. The handler treats this as a 404.
const ENOTDIR_LINE: &str = "ENOTDIR";

/// Returns true iff the captured stdout is the ENOTDIR sentinel.
#[must_use]
pub fn is_enotdir_sentinel(s: &str) -> bool {
    s.lines().next().map(str::trim) == Some(ENOTDIR_LINE)
}

/// Parses the size field emitted by `stat -c '%s'`. Returns None if the
/// stdout is non-numeric (file does not exist).
#[must_use]
pub fn parse_stat_size(s: &str) -> Option<u64> {
    s.trim().parse::<u64>().ok()
}

/// Parses the tab-delimited output of [`LIST_SCRIPT`] into entries.
/// Malformed lines are silently skipped — defensive for busybox quirks.
#[must_use]
pub fn parse_list_output(s: &str) -> Vec<FileEntry> {
    s.lines().filter_map(parse_list_line).collect()
}

fn parse_list_line(line: &str) -> Option<FileEntry> {
    let mut parts = line.splitn(4, '\t');
    let ty = parts.next()?;
    let size = parts.next()?.parse::<u64>().ok()?;
    let mtime = parts.next()?.parse::<i64>().ok()?;
    let name = parts.next()?;
    if name.is_empty() {
        return None;
    }
    let entry_type = match ty {
        "f" => FileEntryType::F,
        "d" => FileEntryType::D,
        "l" => FileEntryType::L,
        _ => FileEntryType::O,
    };
    Some(FileEntry {
        name: name.to_owned(),
        entry_type,
        size,
        mtime,
    })
}

/// Shell script the file-browser execs to enumerate a directory.
///
/// Runs as `sh -c $LIST_SCRIPT _ /data/<safe-path>`. Emits one line per
/// entry: `type<TAB>size<TAB>mtime<TAB>name`. Skips `.` / `..`. On a
/// missing or non-directory target, prints the ENOTDIR sentinel and
/// exits non-zero.
pub const LIST_SCRIPT: &str = r#"cd "$1" 2>/dev/null || { printf 'ENOTDIR\n'; exit 1; }
for entry in * .*; do
  [ "$entry" = "." ] && continue
  [ "$entry" = ".." ] && continue
  [ -e "$entry" ] || [ -L "$entry" ] || continue
  if [ -L "$entry" ]; then ty=l
  elif [ -d "$entry" ]; then ty=d
  elif [ -f "$entry" ]; then ty=f
  else ty=o
  fi
  if [ "$ty" = "f" ] || [ "$ty" = "l" ]; then
    sz=$(stat -c '%s' "$entry" 2>/dev/null || printf '0')
  else
    sz=0
  fi
  mt=$(stat -c '%Y' "$entry" 2>/dev/null || printf '0')
  printf '%s\t%s\t%s\t%s\n' "$ty" "$sz" "$mt" "$entry"
done
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn list_parses_empty_dir() {
        let out = parse_list_output("");
        assert_eq!(out, Vec::<FileEntry>::new());
    }

    #[test]
    fn list_parses_single_file() {
        let out = parse_list_output("f\t1234\t1714000000\tlevel.dat\n");
        assert_eq!(
            out,
            vec![FileEntry {
                name: "level.dat".into(),
                entry_type: FileEntryType::F,
                size: 1234,
                mtime: 1_714_000_000,
            }]
        );
    }

    #[test]
    fn list_parses_dir_with_zero_size() {
        let out = parse_list_output("d\t0\t1714000000\tregion\n");
        assert_eq!(out[0].entry_type, FileEntryType::D);
        assert_eq!(out[0].size, 0);
    }

    #[test]
    fn list_parses_hidden() {
        let out = parse_list_output("d\t0\t1714000000\t.cache\n");
        assert_eq!(out[0].name, ".cache");
    }

    #[test]
    fn list_parses_symlink() {
        let out = parse_list_output("l\t32\t1714000000\told.jar.disabled\n");
        assert_eq!(out[0].entry_type, FileEntryType::L);
        assert_eq!(out[0].size, 32);
    }

    #[test]
    fn list_parses_other_type() {
        let out = parse_list_output("o\t0\t1714000000\tsome_socket\n");
        assert_eq!(out[0].entry_type, FileEntryType::O);
    }

    #[test]
    fn list_parses_name_with_spaces() {
        let out = parse_list_output("f\t100\t1714000000\tWorld 2.zip\n");
        assert_eq!(out[0].name, "World 2.zip");
    }

    #[test]
    fn list_skips_malformed_lines() {
        let out = parse_list_output("garbage no tabs\nf\t1\t0\ta\nalsomalformed\nf\t2\t0\tb\n");
        let names: Vec<&str> = out.iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, vec!["a", "b"]);
    }

    #[test]
    fn list_handles_unknown_type_byte_as_other() {
        let out = parse_list_output("z\t0\t0\tweird\n");
        assert_eq!(out[0].entry_type, FileEntryType::O);
    }

    #[test]
    fn stat_size_parses_numeric() {
        assert_eq!(parse_stat_size("1234\n"), Some(1234));
        assert_eq!(parse_stat_size("0"), Some(0));
    }

    #[test]
    fn stat_size_returns_none_on_garbage() {
        assert_eq!(parse_stat_size(""), None);
        assert_eq!(parse_stat_size("not a number"), None);
    }

    #[test]
    fn enotdir_sentinel_detected() {
        assert!(is_enotdir_sentinel("ENOTDIR"));
        assert!(is_enotdir_sentinel("ENOTDIR\n"));
        assert!(is_enotdir_sentinel("ENOTDIR\nextra\n"));
        assert!(!is_enotdir_sentinel(""));
        assert!(!is_enotdir_sentinel("not-a-sentinel\n"));
    }
}
