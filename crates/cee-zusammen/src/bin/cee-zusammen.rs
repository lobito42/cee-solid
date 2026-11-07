//! `cee-zusammen`: work on one design together, from two terminals.
//!
//! ```text
//! cee-zusammen serve [--port 7878] --teil winkelhalter=crates/cee-aenderung/examples/winkelhalter.sc
//! cee-zusammen verbinde winkelhalter --name Anna [--server 127.0.0.1:7878] [--zuschauen]
//! ```
//!
//! After `verbinde`, type `hilfe` for the commands.

use std::io::BufRead;
use std::process::ExitCode;
use std::sync::mpsc::{Sender, channel};
use std::thread;

use cee_zusammen::netz::{Ereignis, Leitung, starte};
use cee_zusammen::protokoll::AnServer;
use cee_zusammen::{Client, Server};
use serde_json::{Value, json};
use solidcraft_doc::Document;

const HILFE: &str = "\
Befehle:
  stand                         Größen, Schritte, wer dabei ist
  groesse <name> <formel>       eine Größe setzen, z. B.: groesse dicke 10 mm
  befehl <befehl> [json]        einen SolidCraft-Befehl ausführen, z. B.:
                                befehl solid.fillet {\"edges\": [[0, 20, 8]], \"radius\": 2}
  loesche <schritt>             einen Schritt löschen (Name oder Nummer)
  rueckgaengig | wiederholen    nur die eigenen Schritte
  skizze <schritt> | skizze fertig
                                in einer Skizze arbeiten (sie ist so lange für andere gesperrt)
  auswahl <text …>              zeigen, was du ausgewählt hast (wird nie gespeichert)
  offline | online              Verbindung trennen und wieder aufnehmen
  ende";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let ergebnis = match args.first().map(String::as_str) {
        Some("serve") => serve(args.get(1..).unwrap_or_default()),
        Some("verbinde") => verbinde(args.get(1..).unwrap_or_default()),
        _ => Err("Aufruf: cee-zusammen serve [--port N] --teil <name>=<datei.sc> …\n       \
                  cee-zusammen verbinde <teil> --name <name> [--server <adresse>] [--zuschauen]"
            .into()),
    };
    match ergebnis {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// The value after `--flag`, if given.
fn wert<'a>(args: &'a [String], flag: &str) -> Option<&'a str> {
    args.iter().position(|a| a == flag).and_then(|i| args.get(i + 1)).map(String::as_str)
}

fn serve(args: &[String]) -> Result<(), String> {
    let port = wert(args, "--port").unwrap_or("7878");
    let mut teile = Vec::new();
    for (i, a) in args.iter().enumerate() {
        if a == "--teil" {
            let spec = args.get(i + 1).ok_or("--teil <name>=<datei.sc>")?;
            let (name, pfad) = spec.split_once('=').ok_or("--teil <name>=<datei.sc>")?;
            let bytes = std::fs::read(pfad).map_err(|e| format!("{pfad}: {e}"))?;
            let teil = solidcraft_io::read_design(&bytes).map_err(|e| format!("{pfad}: {e}"))?;
            teile.push((name.to_owned(), teil));
        }
    }
    if teile.is_empty() {
        return Err("mindestens ein Teil angeben: --teil <name>=<datei.sc>".into());
    }
    let server = Server::neu(teile)?;
    let namen = server.teile().join(", ");
    let laufend = starte(&format!("127.0.0.1:{port}"), server)?;
    println!("cee-zusammen: Server auf {} mit den Teilen: {namen}", laufend.adresse);
    println!("Beenden mit Strg+C. Nichts wird gespeichert; der Stand lebt, solange der Server läuft.");
    loop {
        thread::park();
    }
}

enum Eingang {
    Zeile(String),
    Ende,
    /// From the connection with this number (old connections' last words are ignored).
    Netz(u64, Ereignis),
}

fn oeffne(adresse: &str, tx: &Sender<Eingang>, nummer: u64) -> Result<Leitung, String> {
    Leitung::oeffne(adresse, tx.clone(), move |e| Eingang::Netz(nummer, e))
}

fn verbinde(args: &[String]) -> Result<(), String> {
    let teil = args.first().filter(|a| !a.starts_with("--")).ok_or("cee-zusammen verbinde <teil> --name <name>")?;
    let name = wert(args, "--name").ok_or("--name <name> fehlt")?;
    let adresse = wert(args, "--server").unwrap_or("127.0.0.1:7878").to_owned();
    let zuschauen = args.iter().any(|a| a == "--zuschauen");
    let (tx, rx) = channel::<Eingang>();
    let eingabe = tx.clone();
    thread::spawn(move || {
        for zeile in std::io::stdin().lock().lines() {
            match zeile {
                Ok(z) => {
                    if eingabe.send(Eingang::Zeile(z)).is_err() {
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = eingabe.send(Eingang::Ende);
    });
    let mut client = Client::neu(name, teil);
    let mut nummer = 1;
    let mut leitung = Some(oeffne(&adresse, &tx, nummer)?);
    if let Some(l) = leitung.as_mut() {
        l.sende(&client.hallo())?;
    }
    if !zuschauen {
        println!("Verbunden mit {adresse} als {name}. „hilfe“ zeigt die Befehle.");
    }
    let mut verlauf: Vec<String> = Vec::new();
    for e in rx {
        let mut ausgehend: Vec<AnServer> = Vec::new();
        match e {
            Eingang::Ende => break,
            Eingang::Zeile(z) => match befehl(&mut client, z.trim(), &mut ausgehend) {
                Aktion::Weiter => {}
                Aktion::Ende => break,
                Aktion::Offline => {
                    if let Some(l) = leitung.take() {
                        l.schliesse();
                    }
                    client.getrennt();
                }
                Aktion::Online => {
                    if leitung.is_none() {
                        nummer += 1;
                        match oeffne(&adresse, &tx, nummer) {
                            Ok(mut l) => {
                                l.sende(&client.hallo())?;
                                leitung = Some(l);
                            }
                            Err(e) => println!("Keine Verbindung: {e}"),
                        }
                    }
                }
            },
            Eingang::Netz(n, _) if n != nummer => {}
            Eingang::Netz(_, Ereignis::Nachricht(m)) => client.empfange(m),
            Eingang::Netz(_, Ereignis::Getrennt) => {
                if leitung.take().is_some() {
                    client.getrennt();
                    println!("Die Verbindung ist weg; „online“ verbindet wieder.");
                }
            }
        }
        if let Some(l) = leitung.as_mut() {
            while let Some(m) = client.ausgehend() {
                ausgehend.push(m);
            }
            for m in &ausgehend {
                if let Err(e) = l.sende(m) {
                    println!("Senden ging nicht: {e}");
                }
            }
        }
        let meldungen = client.meldungen();
        if zuschauen {
            verlauf.extend(meldungen);
            let start = verlauf.len().saturating_sub(12);
            verlauf.drain(..start);
            // Clear the terminal and draw the state again.
            print!("\x1b[2J\x1b[H");
            println!("{}", stand(&client));
            println!("Zuletzt:");
            for m in &verlauf {
                println!("  {m}");
            }
        } else {
            for m in meldungen {
                println!("  {m}");
            }
        }
    }
    Ok(())
}

enum Aktion {
    Weiter,
    Ende,
    Offline,
    Online,
}

fn befehl(c: &mut Client, z: &str, aus: &mut Vec<AnServer>) -> Aktion {
    let (wort, rest) = z.split_once(' ').map_or((z, ""), |(w, r)| (w, r.trim()));
    let zeige = |r: Result<Vec<String>, String>| match r {
        Ok(text) if text.is_empty() => println!("  (nichts geändert)"),
        Ok(text) => text.iter().for_each(|t| println!("  {t}")),
        Err(e) => println!("  Geht nicht: {e}"),
    };
    match wort {
        "" => {}
        "hilfe" | "?" => println!("{HILFE}"),
        "ende" | "exit" | "quit" => return Aktion::Ende,
        "stand" => println!("{}", stand(c)),
        "groesse" => match rest.split_once(' ') {
            Some((name, formel)) => zeige(c.befehl("parameters.change", &json!({"name": name, "expression": formel.trim()}))),
            None => println!("  groesse <name> <formel>"),
        },
        "befehl" => {
            let (id, params) = rest.split_once(' ').map_or((rest, "{}"), |(i, p)| (i, p));
            match serde_json::from_str::<Value>(params) {
                Ok(p) if !id.is_empty() => zeige(c.befehl(id, &p)),
                Ok(_) => println!("  befehl <befehl> [json]"),
                Err(e) => println!("  Die Parameter sind kein JSON: {e}"),
            }
        }
        "loesche" => match schritt(c.teil(), rest) {
            Some(id) => {
                let name = c.teil().feature(id).map(|f| f.name.clone()).unwrap_or_default();
                zeige(c.befehl("timeline.delete", &json!({"features": [name]})));
            }
            None => println!("  Diesen Schritt gibt es nicht: {rest}"),
        },
        "rueckgaengig" | "undo" => zeige(c.rueckgaengig()),
        "wiederholen" | "redo" => zeige(c.wiederholen()),
        "skizze" if rest == "fertig" || rest == "aus" => aus.push(c.in_skizze(None)),
        "skizze" => match schritt(c.teil(), rest) {
            Some(id) => aus.push(c.in_skizze(Some(id))),
            None => println!("  Diese Skizze gibt es nicht: {rest}"),
        },
        "auswahl" => aus.push(c.auswahl(rest.split_whitespace().map(str::to_owned).collect())),
        "offline" => return Aktion::Offline,
        "online" => return Aktion::Online,
        _ => println!("  Unbekannter Befehl „{wort}“. „hilfe“ zeigt die Befehle."),
    }
    Aktion::Weiter
}

/// A step by its number or name.
fn schritt(d: &Document, s: &str) -> Option<u64> {
    let s = s.trim();
    if let Ok(id) = s.parse::<u64>()
        && d.feature(id).is_some()
    {
        return Some(id);
    }
    d.features.iter().find(|f| f.name == s).map(|f| f.id)
}

fn art(f: &solidcraft_doc::Feature) -> String {
    serde_json::to_value(&f.kind).ok().and_then(|v| v.get("type").and_then(Value::as_str).map(str::to_owned)).unwrap_or_default()
}

fn stand(c: &Client) -> String {
    let d = c.teil();
    let (_, seq) = c.bestaetigt();
    let mut s = String::new();
    let online = if c.ist_verbunden() { "online" } else { "offline" };
    s.push_str(&format!("Stand {seq}, {online}, {} eigene Änderung(en) noch nicht bestätigt\n", c.offen()));
    s.push_str("Größen:\n");
    for p in &d.params {
        s.push_str(&format!("  {} = {}\n", p.name, p.expr));
    }
    s.push_str("Schritte:\n");
    for f in &d.features {
        let unterdrueckt = if f.suppressed { " (unterdrückt)" } else { "" };
        let wer: Vec<&str> = c.personen().filter(|p| p.skizze == Some(f.id)).map(|p| p.name.as_str()).collect();
        let gesperrt = if wer.is_empty() {
            if c.skizze() == Some(f.id) { " [du arbeitest darin]".to_owned() } else { String::new() }
        } else {
            format!(" [gesperrt: {} arbeitet darin]", wer.join(", "))
        };
        s.push_str(&format!("  {:>3} {} ({}){unterdrueckt}{gesperrt}\n", f.id, f.name, art(f)));
    }
    let personen: Vec<String> = c
        .personen()
        .map(|p| {
            let auswahl = if p.auswahl.is_empty() { String::new() } else { format!(", Auswahl: {}", p.auswahl.join(" ")) };
            format!("{}{auswahl}", p.name)
        })
        .collect();
    let dabei = if personen.is_empty() { "niemand sonst".to_owned() } else { personen.join("; ") };
    s.push_str(&format!("Dabei: {dabei}"));
    s
}
