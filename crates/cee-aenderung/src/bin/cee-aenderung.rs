//! `cee-aenderung`: change sets between SolidCraft designs on the command line.
//!
//! ```text
//! cee-aenderung diff  vorher.sc nachher.sc [-o satz.json]   change set as JSON (stdout or file)
//! cee-aenderung apply basis.sc satz.json -o neu.sc          apply it (all or nothing)
//! cee-aenderung lies  satz.json [basis.sc]                  read it as German sentences
//! cee-aenderung fingerabdruck teil.sc                       the design's fingerprint
//! ```
//!
//! Designs are read with SolidCraft's own reader (a `.sc` package or plain JSON). `apply` writes a
//! package for `.sc` and plain JSON for any other name.

use std::path::Path;
use std::process::ExitCode;

use cee_aenderung::{Fehler, berechne, fingerabdruck, lies, satz_als_json, satz_aus_json, wende_an};
use solidcraft_doc::Document;

const HILFE: &str = "\
cee-aenderung: Änderungssätze zwischen zwei Ständen eines SolidCraft-Teils

  cee-aenderung diff  vorher.sc nachher.sc [-o satz.json]
  cee-aenderung apply basis.sc satz.json -o neu.sc
  cee-aenderung lies  satz.json [basis.sc]
  cee-aenderung fingerabdruck teil.sc
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("Fehler: {e}");
            ExitCode::FAILURE
        }
    }
}

fn run(args: &[String]) -> Result<(), String> {
    let (pos, out) = split_out(args)?;
    match pos.iter().map(String::as_str).collect::<Vec<_>>().as_slice() {
        ["diff", a, b] => {
            let satz = berechne(&teil(a)?, &teil(b)?).map_err(text)?;
            let json = satz_als_json(&satz).map_err(text)?;
            match out {
                Some(o) => std::fs::write(&o, json).map_err(|e| format!("{o}: {e}")),
                None => {
                    print!("{json}");
                    Ok(())
                }
            }
        }
        ["apply", basis, satz] => {
            let Some(o) = out else { return Err("apply braucht -o <datei>".into()) };
            let bytes = std::fs::read(satz).map_err(|e| format!("{satz}: {e}"))?;
            let neu = wende_an(&teil(basis)?, &satz_aus_json(&bytes).map_err(text)?).map_err(text)?;
            let data = if Path::new(&o).extension().is_some_and(|e| e.eq_ignore_ascii_case("sc")) {
                solidcraft_io::write_design_container(&neu).map_err(|e| e.to_string())?
            } else {
                solidcraft_io::write_design(&neu)
            };
            std::fs::write(&o, data).map_err(|e| format!("{o}: {e}"))?;
            eprintln!("geschrieben: {o}");
            Ok(())
        }
        ["lies", satz, rest @ ..] if rest.len() <= 1 && out.is_none() => {
            let bytes = std::fs::read(satz).map_err(|e| format!("{satz}: {e}"))?;
            let s = satz_aus_json(&bytes).map_err(text)?;
            let basis = match rest.first() {
                Some(b) => Some(teil(b)?),
                None => None,
            };
            println!("{} Änderung(en), Ausgangsstand {}", s.aenderungen.len(), kurz_hash(&s.ausgangsstand));
            for zeile in lies(&s, basis.as_ref()) {
                println!("- {zeile}");
            }
            Ok(())
        }
        ["fingerabdruck", t] if out.is_none() => {
            println!("{}", fingerabdruck(&teil(t)?).map_err(text)?);
            Ok(())
        }
        _ => Err(format!("unbekannter Aufruf\n\n{HILFE}")),
    }
}

/// The positional arguments and the value of `-o`.
fn split_out(args: &[String]) -> Result<(Vec<String>, Option<String>), String> {
    let (mut pos, mut out) = (Vec::new(), None);
    let mut it = args.iter();
    while let Some(a) = it.next() {
        match a.as_str() {
            "-o" | "--out" => out = Some(it.next().ok_or("-o braucht einen Dateinamen")?.clone()),
            "-h" | "--help" => return Err(HILFE.into()),
            _ => pos.push(a.clone()),
        }
    }
    Ok((pos, out))
}

fn teil(path: &str) -> Result<Document, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    solidcraft_io::read_design(&bytes).map_err(|e| format!("{path}: {e}"))
}

fn text(e: Fehler) -> String {
    e.to_string()
}

fn kurz_hash(h: &str) -> String {
    h.chars().take(19).collect()
}
