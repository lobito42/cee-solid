//! Moving the messages over TCP, one JSON line per message. Plain threads and blocking sockets,
//! no async runtime (as FigCraft's network crate does it, see ATTRIBUTION).
//!
//! Local use only for now: the server listens where it is told (the command line binds to
//! 127.0.0.1); there are no accounts, links or encryption yet.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

use crate::protokoll::{AnClient, AnServer, MAX_ZEILE, lies_zeile, zeile};
use crate::server::{Ausgang, Server, Verbindung};

fn sperre<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    match m.lock() {
        Ok(g) => g,
        Err(vergiftet) => vergiftet.into_inner(),
    }
}

/// One line from `r` (without the newline); `Ok(None)` at the end; refused when longer than
/// [`MAX_ZEILE`].
fn naechste_zeile(r: &mut impl BufRead) -> std::io::Result<Option<Vec<u8>>> {
    let mut buf = Vec::new();
    let n = r.by_ref().take(MAX_ZEILE as u64 + 1).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() == Some(&b'\n') {
        buf.pop();
    } else if buf.len() > MAX_ZEILE {
        return Err(std::io::Error::new(std::io::ErrorKind::InvalidData, "Nachricht zu groß"));
    }
    Ok(Some(buf))
}

struct Netz {
    server: Mutex<Server>,
    schreiber: Mutex<BTreeMap<Verbindung, Sender<String>>>,
}

impl Netz {
    fn verteile(&self, out: Vec<Ausgang>) {
        let schreiber = sperre(&self.schreiber);
        for (v, n) in out {
            if let (Some(tx), Ok(z)) = (schreiber.get(&v), zeile(&n)) {
                let _ = tx.send(z);
            }
        }
    }
}

/// A running server: where it listens.
pub struct Laufend {
    pub adresse: SocketAddr,
}

/// Listen on `adresse` and serve in background threads. Returns once listening.
pub fn starte(adresse: &str, server: Server) -> Result<Laufend, String> {
    let listener = TcpListener::bind(adresse).map_err(|e| format!("{adresse}: {e}"))?;
    let adresse = listener.local_addr().map_err(|e| e.to_string())?;
    let netz = Arc::new(Netz { server: Mutex::new(server), schreiber: Mutex::new(BTreeMap::new()) });
    thread::Builder::new()
        .name("cee-zusammen-annehmen".into())
        .spawn(move || {
            for (v, stream) in (1..).zip(listener.incoming().flatten()) {
                let v: Verbindung = v;
                let netz = netz.clone();
                let _ = thread::Builder::new().name(format!("cee-zusammen-{v}")).spawn(move || verbindung(&netz, v, stream));
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(Laufend { adresse })
}

fn verbindung(netz: &Netz, v: Verbindung, stream: TcpStream) {
    let _ = stream.set_nodelay(true);
    let Ok(mut schreib) = stream.try_clone() else { return };
    let (tx, rx) = channel::<String>();
    sperre(&netz.schreiber).insert(v, tx);
    let schreiber = thread::Builder::new().name(format!("cee-zusammen-{v}-schreiben")).spawn(move || {
        for z in rx {
            if schreib.write_all(z.as_bytes()).is_err() {
                break;
            }
        }
        let _ = schreib.shutdown(Shutdown::Both);
    });
    let mut r = BufReader::new(stream);
    loop {
        match naechste_zeile(&mut r) {
            Ok(Some(z)) if z.iter().all(u8::is_ascii_whitespace) => {}
            Ok(Some(z)) => {
                // Answers go out while the server is held, so everyone gets them in its order.
                let mut server = sperre(&netz.server);
                let out = match lies_zeile::<AnServer>(&z) {
                    Ok(n) => server.empfange(v, n),
                    Err(grund) => vec![(v, AnClient::Abgewiesen { grund })],
                };
                netz.verteile(out);
            }
            Ok(None) | Err(_) => break,
        }
    }
    {
        let mut server = sperre(&netz.server);
        let out = server.getrennt(v);
        sperre(&netz.schreiber).remove(&v);
        netz.verteile(out);
    }
    if let Ok(h) = schreiber {
        let _ = h.join();
    }
}

/// What a client's connection reports.
#[derive(Debug)]
pub enum Ereignis {
    Nachricht(AnClient),
    /// The connection is gone.
    Getrennt,
}

/// A client's connection to the server.
pub struct Leitung {
    stream: TcpStream,
}

impl Leitung {
    /// Connect to `adresse`; what arrives goes to `an` (wrapped by `huelle`, so a program can put
    /// the server's messages and its own input on one channel).
    pub fn oeffne<E: Send + 'static>(adresse: &str, an: Sender<E>, huelle: impl Fn(Ereignis) -> E + Send + 'static) -> Result<Self, String> {
        let stream = TcpStream::connect(adresse).map_err(|e| format!("{adresse}: {e}"))?;
        let _ = stream.set_nodelay(true);
        let lesen = stream.try_clone().map_err(|e| e.to_string())?;
        thread::Builder::new()
            .name("cee-zusammen-empfangen".into())
            .spawn(move || {
                let mut r = BufReader::new(lesen);
                while let Ok(Some(z)) = naechste_zeile(&mut r) {
                    let e = match lies_zeile::<AnClient>(&z) {
                        Ok(n) => Ereignis::Nachricht(n),
                        Err(grund) => Ereignis::Nachricht(AnClient::Abgewiesen { grund }),
                    };
                    if an.send(huelle(e)).is_err() {
                        return;
                    }
                }
                let _ = an.send(huelle(Ereignis::Getrennt));
            })
            .map_err(|e| e.to_string())?;
        Ok(Self { stream })
    }

    pub fn sende(&mut self, n: &AnServer) -> Result<(), String> {
        let z = zeile(n)?;
        self.stream.write_all(z.as_bytes()).map_err(|e| e.to_string())
    }

    /// Close the connection (the reading thread then reports [`Ereignis::Getrennt`]).
    pub fn schliesse(&self) {
        let _ = self.stream.shutdown(Shutdown::Both);
    }
}

/// A channel for a client's events, for programs that need only the server's messages.
pub fn kanal() -> (Sender<Ereignis>, Receiver<Ereignis>) {
    channel()
}
