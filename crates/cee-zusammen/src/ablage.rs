//! Keeping the shared state when the server ends: it goes into a new file next to the file the
//! part was loaded from (`<name>.<time>.sc`), never over that file.

use std::path::{Path, PathBuf};

use solidcraft_doc::Document;

/// The date and time (UTC) of `sekunden` since 1970 as `YYYYMMDD-HHMMSS`.
pub fn zeitstempel(sekunden: u64) -> String {
    let tage = (sekunden / 86_400) as i64;
    let rest = sekunden % 86_400;
    let (j, m, t) = datum(tage);
    format!("{j:04}{m:02}{t:02}-{:02}{:02}{:02}", rest / 3600, rest / 60 % 60, rest % 60)
}

/// Year, month, day of `tage` days after 1970-01-01 (the civil calendar, proleptic Gregorian).
fn datum(tage: i64) -> (i64, u32, u32) {
    let z = tage + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = yoe + era * 400 + i64::from(m <= 2);
    (y, m, d)
}

/// A file name next to `ausgang` for the state at `sekunden`: `<stem>.<time>.sc`, with `-2`,
/// `-3` … when that name is taken. Never `ausgang` itself.
pub fn neuer_pfad(ausgang: &Path, sekunden: u64) -> PathBuf {
    let stamm = ausgang.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "teil".into());
    let ordner = ausgang.parent().unwrap_or_else(|| Path::new(""));
    let zeit = zeitstempel(sekunden);
    let mut n = 1u32;
    loop {
        let name = if n == 1 { format!("{stamm}.{zeit}.sc") } else { format!("{stamm}.{zeit}-{n}.sc") };
        let p = ordner.join(name);
        if !p.exists() && p != ausgang {
            return p;
        }
        n += 1;
    }
}

/// Write `teil` as a SolidCraft package to `pfad` (through a temporary file, so a half-written
/// file never carries the name).
pub fn schreibe(pfad: &Path, teil: &Document) -> Result<(), String> {
    let data = solidcraft_io::write_design_container(teil).map_err(|e| e.to_string())?;
    let tmp = pfad.with_extension("sc.tmp");
    std::fs::write(&tmp, data).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, pfad).map_err(|e| format!("{}: {e}", pfad.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_stamps_are_calendar_dates() {
        assert_eq!(zeitstempel(0), "19700101-000000");
        assert_eq!(zeitstempel(951_782_400), "20000229-000000");
        assert_eq!(zeitstempel(1_794_052_245), "20261107-115045");
    }

    #[test]
    fn the_new_name_never_is_the_source_and_never_taken() {
        let dir = std::env::temp_dir().join(format!("cee-ablage-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let ausgang = dir.join("winkel.sc");
        std::fs::write(&ausgang, b"x").unwrap();
        let a = neuer_pfad(&ausgang, 0);
        assert_eq!(a, dir.join("winkel.19700101-000000.sc"));
        std::fs::write(&a, b"y").unwrap();
        assert_eq!(neuer_pfad(&ausgang, 0), dir.join("winkel.19700101-000000-2.sc"));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
