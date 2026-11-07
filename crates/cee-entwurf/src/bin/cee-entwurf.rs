//! `cee-entwurf`: drafts of a SolidCraft design offline, and merging them later.
//!
//! ```text
//! cee-entwurf neu <ordner> <teil.sc> [--autor <name>]
//! cee-entwurf sichern <ordner> <entwurf> <teil.sc> [--autor <name>]
//! cee-entwurf verzweigen <ordner> <von> <neu>
//! cee-entwurf stand <ordner> <entwurf> -o <teil.sc>
//! cee-entwurf verlauf <ordner> <entwurf>
//! cee-entwurf entwuerfe <ordner>
//! cee-entwurf zusammenfuehren <ordner> <ein> <in> [--autor <name>] [--nimm ein|in|<nr>=ein|<nr>=in]… [--spaeterer-gewinnt] [-o <teil.sc>]
//! cee-entwurf holen <ordner> <anderer-ordner>
//! cee-entwurf pruefen <ordner>
//! cee-entwurf pruefe <teil.sc>
//! cee-entwurf bearbeiten <teil.sc> <befehl> [json]
//! ```
//!
//! The author is `--autor`, else the environment variable `CEE_AUTOR`, else "unbekannt". The time
//! of each entry is recorded unless `--ohne-zeit` is given.

use std::collections::BTreeMap;
use std::io::{BufRead, IsTerminal, Write};
use std::path::Path;
use std::process::ExitCode;

use cee_entwurf::verlauf::{Verlauf, kurz};
use cee_entwurf::zusammen::{Konflikt, Seite};
use cee_entwurf::{Ausgang, Staende, Wahl, pruefer};
use serde_json::Value;
use solidcraft_doc::Document;

const HILFE: &str = "\
cee-entwurf: Entwürfe eines Teils offline, später zusammenführen

  cee-entwurf neu <ordner> <teil.sc>                 Verlauf anlegen, Entwurf „haupt“
  cee-entwurf sichern <ordner> <entwurf> <teil.sc>   den Stand des Teils als nächsten Satz sichern
  cee-entwurf verzweigen <ordner> <von> <neu>        neuer Entwurf am selben Stand
  cee-entwurf stand <ordner> <entwurf> -o <teil.sc>  das Teil an diesem Stand herstellen
  cee-entwurf verlauf <ordner> <entwurf>             wer was geändert hat
  cee-entwurf entwuerfe <ordner>                     die Entwürfe
  cee-entwurf zusammenfuehren <ordner> <ein> <in>    „ein“ in „in“ zusammenführen
        [--nimm ein|in]  [--nimm <nr>=ein|in]…  [--spaeterer-gewinnt]  [-o <teil.sc>]
  cee-entwurf holen <ordner> <anderer-ordner>        Sätze und Entwürfe aus dem anderen Ordner holen
  cee-entwurf pruefen <ordner>                       alle Dateien des Verlaufs prüfen
  cee-entwurf pruefe <teil.sc>                       das Teil neu aufbauen und prüfen
  cee-entwurf bearbeiten <teil.sc> <befehl> [json]   einen SolidCraft-Befehl auf das Teil anwenden

  --autor <name> (sonst CEE_AUTOR), --ohne-zeit: keine Uhrzeit im Satz
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match run(&args) {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Fehler: {e}");
            ExitCode::FAILURE
        }
    }
}

struct Optionen {
    pos: Vec<String>,
    autor: Option<String>,
    out: Option<String>,
    nimm: Vec<String>,
    spaeter: bool,
    ohne_zeit: bool,
}

fn optionen(args: &[String]) -> Result<Optionen, String> {
    let mut o = Optionen { pos: Vec::new(), autor: None, out: None, nimm: Vec::new(), spaeter: false, ohne_zeit: false };
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut wert = |f: &str| it.next().cloned().ok_or(format!("{f} braucht einen Wert"));
        match a.as_str() {
            "--autor" => o.autor = Some(wert("--autor")?),
            "-o" | "--out" => o.out = Some(wert("-o")?),
            "--nimm" => o.nimm.push(wert("--nimm")?),
            "--spaeterer-gewinnt" => o.spaeter = true,
            "--ohne-zeit" => o.ohne_zeit = true,
            "-h" | "--help" | "hilfe" => return Err(HILFE.into()),
            s if s.starts_with("--") => return Err(format!("unbekannte Option {s}\n\n{HILFE}")),
            _ => o.pos.push(a.clone()),
        }
    }
    Ok(o)
}

fn run(args: &[String]) -> Result<ExitCode, String> {
    let o = optionen(args)?;
    let autor = o.autor.clone().or_else(|| std::env::var("CEE_AUTOR").ok()).unwrap_or_else(|| "unbekannt".into());
    let zeit = if o.ohne_zeit { None } else { cee_entwurf::jetzt() };
    let pos: Vec<&str> = o.pos.iter().map(String::as_str).collect();
    let t = |e: cee_entwurf::Fehler| e.to_string();
    match pos.as_slice() {
        ["neu", ordner, teil_pfad] => {
            let teil = lies_teil(teil_pfad)?;
            let h = cee_entwurf::neu(Path::new(ordner), &teil, &autor, zeit).map_err(t)?;
            println!(
                "Verlauf angelegt in {ordner}: Entwurf „haupt“ = Satz {} ({} Größen, {} Schritte)",
                kurz(&h),
                teil.params.len(),
                teil.features.len()
            );
        }
        ["sichern", ordner, entwurf, teil_pfad] => {
            let v = oeffne(ordner)?;
            let teil = lies_teil(teil_pfad)?;
            match cee_entwurf::sichern(&v, entwurf, &teil, &autor, zeit).map_err(t)? {
                None => println!("Nichts zu sichern: „{entwurf}“ hat schon diesen Stand."),
                Some((h, text)) => {
                    println!("Gesichert in „{entwurf}“: Satz {} von {autor}", kurz(&h));
                    for z in text {
                        println!("  - {z}");
                    }
                }
            }
        }
        ["verzweigen", ordner, von, neu] => {
            let v = oeffne(ordner)?;
            cee_entwurf::verzweigen(&v, von, neu).map_err(t)?;
            println!("Neuer Entwurf „{neu}“ am Stand von „{von}“.");
        }
        ["stand", ordner, entwurf] => {
            let Some(out) = &o.out else { return Err("stand braucht -o <teil.sc>".into()) };
            let v = oeffne(ordner)?;
            let h = v.entwurf(entwurf).map_err(t)?;
            let teil = Staende::default().stand(&v, &h).map_err(t)?;
            schreibe_teil(out, &teil)?;
            println!("„{entwurf}“ (Satz {}) geschrieben: {out}", kurz(&h));
        }
        ["verlauf", ordner, entwurf] => {
            let v = oeffne(ordner)?;
            let h = v.entwurf(entwurf).map_err(t)?;
            verlauf(&v, entwurf, &h)?;
        }
        ["entwuerfe", ordner] => {
            let v = oeffne(ordner)?;
            for (name, h) in v.entwuerfe().map_err(t)? {
                println!("{name}  (Satz {})", kurz(&h));
            }
        }
        ["zusammenfuehren", ordner, ein, nach] => return zusammenfuehren(&o, ordner, ein, nach, &autor, zeit),
        ["holen", ordner, andere] => {
            let v = oeffne(ordner)?;
            let a = oeffne(andere)?;
            let quelle = Path::new(andere)
                .canonicalize()
                .ok()
                .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
                .unwrap_or_else(|| "anderer".into());
            let (n, gesetzt, ausgelassen) = v.hole(&a, &quelle).map_err(t)?;
            println!("{n} Satz/Sätze aus {andere} geholt.");
            for g in gesetzt {
                println!("  Entwurf „{g}“");
            }
            for a in ausgelassen {
                println!("  ausgelassen: {a}");
            }
        }
        ["pruefen", ordner] => {
            let v = oeffne(ordner)?;
            let befunde = v.pruefe_dateien().map_err(t)?;
            if befunde.is_empty() {
                println!("Alle Dateien in Ordnung ({} Sätze).", v.alle().map_err(t)?.len());
            } else {
                for b in &befunde {
                    println!("{b}");
                }
                return Ok(ExitCode::from(2));
            }
        }
        ["pruefe", teil_pfad] => {
            let b = pruefer::pruefe(&lies_teil(teil_pfad)?);
            for z in &b.zeilen {
                println!("{z}");
            }
            if !b.ok {
                return Ok(ExitCode::from(2));
            }
        }
        ["bearbeiten", teil_pfad, befehl, rest @ ..] if rest.len() <= 1 => {
            let params: Value = match rest.first() {
                Some(p) => serde_json::from_str(p).map_err(|e| format!("die Parameter sind kein JSON: {e}"))?,
                None => Value::Object(Default::default()),
            };
            let vorher = lies_teil(teil_pfad)?;
            let mut s = solidcraft_engine::Session::new(vorher.clone());
            s.execute(befehl, &params).map_err(|e| e.to_string())?;
            if s.active_sketch.is_some() {
                s.execute("sketch.finish", &Value::Object(Default::default())).map_err(|e| e.to_string())?;
            }
            let nachher = (*s.doc).clone();
            let satz = cee_aenderung::berechne(&vorher, &nachher).map_err(|e| e.to_string())?;
            schreibe_teil(teil_pfad, &nachher)?;
            for z in cee_aenderung::lies(&satz, Some(&vorher)) {
                println!("  - {z}");
            }
        }
        _ => return Err(format!("unbekannter Aufruf\n\n{HILFE}")),
    }
    Ok(ExitCode::SUCCESS)
}

fn oeffne(ordner: &str) -> Result<Verlauf, String> {
    Verlauf::oeffnen(Path::new(ordner)).map_err(|e| e.to_string())
}

fn lies_teil(pfad: &str) -> Result<Document, String> {
    let bytes = std::fs::read(pfad).map_err(|e| format!("{pfad}: {e}"))?;
    solidcraft_io::read_design(&bytes).map_err(|e| format!("{pfad}: {e}"))
}

fn schreibe_teil(pfad: &str, teil: &Document) -> Result<(), String> {
    let data = if Path::new(pfad).extension().is_some_and(|e| e.eq_ignore_ascii_case("sc")) {
        solidcraft_io::write_design_container(teil).map_err(|e| e.to_string())?
    } else {
        solidcraft_io::write_design(teil)
    };
    std::fs::write(pfad, data).map_err(|e| format!("{pfad}: {e}"))
}

/// A time as `YYYY-MM-DD HH:MM UTC`.
fn uhrzeit(s: u64) -> String {
    let tage = (s / 86_400) as i64;
    let z = tage + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02} {:02}:{:02} UTC", s % 86_400 / 3600, s % 3600 / 60)
}

fn verlauf(v: &Verlauf, entwurf: &str, h: &str) -> Result<(), String> {
    let t = |e: cee_entwurf::Fehler| e.to_string();
    let mut staende = Staende::default();
    let eintraege = v.reihenfolge(h).map_err(t)?;
    println!("Verlauf von „{entwurf}“, neuester Satz zuerst ({} Sätze):", eintraege.len());
    for (x, e) in eintraege.iter().rev() {
        let wann = e.zeit.map(|z| format!(", {}", uhrzeit(z))).unwrap_or_default();
        println!();
        match (&e.zusammenfuehrung, e.vorgaenger.first()) {
            (Some(z), _) => println!("Satz {} – {}{wann}: „{}“ in „{}“ zusammengeführt", kurz(x), e.autor, z.ein, z.nach),
            (None, None) => println!("Satz {} – {}{wann}: angelegt", kurz(x), e.autor),
            (None, Some(_)) => println!("Satz {} – {}{wann}:", kurz(x), e.autor),
        }
        if e.vorgaenger.is_empty() {
            let teil = staende.stand(v, x).map_err(t)?;
            println!("  - {} Größen, {} Schritte", teil.params.len(), teil.features.len());
        } else {
            let vorher = match e.vorgaenger.first() {
                Some(p) => Some(staende.stand(v, p).map_err(t)?),
                None => None,
            };
            for z in cee_aenderung::lies(&e.satz, vorher.as_ref()) {
                println!("  - {z}");
            }
        }
        if let Some(z) = &e.zusammenfuehrung {
            for d in &z.entscheidungen {
                println!("  Entschieden: {d}");
            }
            for p in &z.pruefer {
                println!("  {p}");
            }
        }
    }
    Ok(())
}

fn zusammenfuehren(o: &Optionen, ordner: &str, ein: &str, nach: &str, autor: &str, zeit: Option<u64>) -> Result<ExitCode, String> {
    let v = oeffne(ordner)?;
    let mut fragen = |k: &Konflikt, ein: &str, nach: &str| -> Option<Seite> {
        println!("Konflikt {}", k.zeile(ein, nach));
        loop {
            print!("  Welche Seite? [e] „{ein}“, [i] „{nach}“, [a] abbrechen: ");
            let _ = std::io::stdout().flush();
            let mut zeile = String::new();
            if std::io::stdin().lock().read_line(&mut zeile).ok()? == 0 {
                return None;
            }
            match zeile.trim() {
                "e" | "ein" => return Some(Seite::Ein),
                "i" | "in" => return Some(Seite::In),
                "a" | "" => return None,
                _ => {}
            }
        }
    };
    let wahl = if o.spaeter {
        Wahl::SpaetererGewinnt
    } else if !o.nimm.is_empty() {
        let mut je = BTreeMap::new();
        let mut sonst = None;
        for n in &o.nimm {
            match n.split_once('=') {
                Some((nr, s)) => {
                    let nr: usize = nr.parse().map_err(|_| format!("--nimm {n}: vor dem = steht die Nummer des Konflikts"))?;
                    je.insert(nr, Seite::parse(s).ok_or(format!("--nimm {n}: ein oder in"))?);
                }
                None => sonst = Some(Seite::parse(n).ok_or(format!("--nimm {n}: ein oder in"))?),
            }
        }
        Wahl::Je { je, sonst }
    } else if std::io::stdin().is_terminal() && std::io::stdout().is_terminal() {
        Wahl::Fragen(&mut fragen)
    } else {
        Wahl::Keine
    };
    let ausgang = cee_entwurf::zusammenfuehren(&v, ein, nach, autor, zeit, wahl).map_err(|e| e.to_string())?;
    match ausgang {
        Ausgang::SchonEnthalten => println!("Nichts zusammenzuführen: „{nach}“ enthält schon alles aus „{ein}“."),
        Ausgang::Vorgespult(h) => {
            println!("„{nach}“ hatte nichts Eigenes dazu: „{nach}“ hat jetzt den Stand von „{ein}“ (Satz {}).", kurz(&h));
            if let Some(out) = &o.out {
                let teil = Staende::default().stand(&v, &h).map_err(|e| e.to_string())?;
                schreibe_teil(out, &teil)?;
                println!("Geschrieben: {out}");
            }
        }
        Ausgang::Konflikte(e) => {
            let offen = e.konflikte.iter().filter(|k| k.gewaehlt.is_none()).count();
            println!("{offen} Konflikt(e), nichts geschrieben:");
            for k in &e.konflikte {
                println!("  {}", k.zeile(ein, nach));
            }
            println!("Entscheiden mit --nimm ein oder --nimm in (alle), --nimm <nr>=ein|in (je Konflikt) oder --spaeterer-gewinnt.");
            return Ok(ExitCode::from(2));
        }
        Ausgang::Zusammengefuehrt { satz, ergebnis, teil, bericht } => {
            println!("„{ein}“ in „{nach}“ zusammengeführt: Satz {}", kurz(&satz));
            for k in &ergebnis.konflikte {
                println!("  Entschieden: {}", k.zeile(ein, nach));
            }
            for h in &ergebnis.hinweise {
                println!("  {h}");
            }
            println!("Prüfbericht:");
            for z in &bericht.zeilen {
                println!("  {z}");
            }
            if let Some(out) = &o.out {
                schreibe_teil(out, &teil)?;
                println!("Geschrieben: {out}");
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}
