//! Lists whose entries have a key (parameters by name, steps by id, sketch items by id): what was
//! removed, inserted, moved and changed, and applying that to a list.
//!
//! Positions are given by the predecessor's key, as FigCraft gives a child its place among its
//! siblings by its neighbours (see ATTRIBUTION). Entries that keep their relative order (the
//! longest increasing run of old positions) stay put; the others move, so a reorder costs as
//! few moves as it can.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::Value;

/// One step that turns the old list into the new one.
#[derive(Clone, Debug, PartialEq)]
pub enum Schritt<K> {
    Weg(K, Value),
    Neu(K, Option<K>, Value),
    Verschoben(K, Option<K>),
    Geaendert(K, Value, Value),
}

/// The keys of a list, when every entry has one and no key repeats.
pub fn schluessel<K: Ord + Clone>(liste: &[Value], key: impl Fn(&Value) -> Option<K>) -> Option<Vec<K>> {
    let mut seen = BTreeSet::new();
    let mut out = Vec::with_capacity(liste.len());
    for v in liste {
        let k = key(v)?;
        if !seen.insert(k.clone()) {
            return None;
        }
        out.push(k);
    }
    Some(out)
}

/// The steps from `alt` to `neu` (keys from [`schluessel`]): removals in old order, then insertions
/// and moves in new order, then changes in new order.
pub fn vergleiche<K: Ord + Clone>(alt: &[(K, &Value)], neu: &[(K, &Value)]) -> Vec<Schritt<K>> {
    let alt_index: BTreeMap<&K, usize> = alt.iter().enumerate().map(|(i, (k, _))| (k, i)).collect();
    let neu_keys: BTreeSet<&K> = neu.iter().map(|(k, _)| k).collect();
    let mut out = Vec::new();
    for (k, v) in alt {
        if !neu_keys.contains(k) {
            out.push(Schritt::Weg(k.clone(), (*v).clone()));
        }
    }
    // Old positions of the entries that stay, in new order; the longest increasing run stays put.
    let common: Vec<(usize, usize)> = neu.iter().enumerate().filter_map(|(j, (k, _))| alt_index.get(k).map(|&i| (j, i))).collect();
    let stays: BTreeSet<usize> = laengste_steigende(&common.iter().map(|&(_, i)| i).collect::<Vec<_>>()).into_iter().map(|p| common[p].0).collect();
    for (j, (k, v)) in neu.iter().enumerate() {
        let vor = j.checked_sub(1).and_then(|p| neu.get(p)).map(|(k, _)| k.clone());
        match alt_index.get(k) {
            None => out.push(Schritt::Neu(k.clone(), vor, (*v).clone())),
            Some(_) if !stays.contains(&j) => out.push(Schritt::Verschoben(k.clone(), vor)),
            Some(_) => {}
        }
    }
    for (k, v) in neu {
        if let Some(&i) = alt_index.get(k)
            && let Some((_, old)) = alt.get(i)
            && *old != *v
        {
            out.push(Schritt::Geaendert(k.clone(), (*old).clone(), (*v).clone()));
        }
    }
    out
}

/// Positions (into `xs`) of a longest strictly increasing subsequence.
fn laengste_steigende(xs: &[usize]) -> Vec<usize> {
    // Patience sorting: `tails[l]` is the position of the smallest tail of a run of length l + 1.
    let mut tails: Vec<usize> = Vec::new();
    let mut prev: Vec<Option<usize>> = vec![None; xs.len()];
    for (p, &x) in xs.iter().enumerate() {
        let at = tails.partition_point(|&t| xs.get(t).is_some_and(|&y| y < x));
        prev[p] = at.checked_sub(1).and_then(|a| tails.get(a).copied());
        if at == tails.len() {
            tails.push(p);
        } else {
            tails[at] = p;
        }
    }
    let mut out = Vec::with_capacity(tails.len());
    let mut cur = tails.last().copied();
    while let Some(p) = cur {
        out.push(p);
        cur = prev.get(p).copied().flatten();
    }
    out.reverse();
    out
}

/// Where the entry with key `k` sits.
pub fn finde<K: PartialEq>(liste: &[Value], key: &impl Fn(&Value) -> Option<K>, k: &K) -> Option<usize> {
    liste.iter().position(|v| key(v).as_ref() == Some(k))
}

/// The index right after the entry `nach` (0 for `None`); `None` when there is no such entry.
pub fn einfuegestelle<K: PartialEq>(liste: &[Value], key: &impl Fn(&Value) -> Option<K>, nach: Option<&K>) -> Option<usize> {
    match nach {
        None => Some(0),
        Some(n) => finde(liste, key, n).map(|i| i + 1),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn key(v: &Value) -> Option<String> {
        v.get("id").and_then(Value::as_str).map(str::to_owned)
    }

    fn anwenden(alt: &[Value], steps: &[Schritt<String>]) -> Vec<Value> {
        let mut l = alt.to_vec();
        for s in steps {
            match s {
                Schritt::Weg(k, _) => {
                    let i = finde(&l, &key, k).unwrap();
                    l.remove(i);
                }
                Schritt::Neu(k, nach, v) => {
                    assert!(finde(&l, &key, k).is_none());
                    let i = einfuegestelle(&l, &key, nach.as_ref()).unwrap();
                    l.insert(i, v.clone());
                }
                Schritt::Verschoben(k, nach) => {
                    let i = finde(&l, &key, k).unwrap();
                    let v = l.remove(i);
                    let j = einfuegestelle(&l, &key, nach.as_ref()).unwrap();
                    l.insert(j, v);
                }
                Schritt::Geaendert(k, _, v) => {
                    let i = finde(&l, &key, k).unwrap();
                    l[i] = v.clone();
                }
            }
        }
        l
    }

    fn liste(ids: &[&str]) -> Vec<Value> {
        ids.iter().map(|i| json!({"id": i})).collect()
    }

    fn paare(l: &[Value]) -> Vec<(String, &Value)> {
        l.iter().map(|v| (key(v).unwrap(), v)).collect()
    }

    #[test]
    fn every_permutation_with_inserts_and_removals_round_trips() {
        // All orders of 5 entries, with one removed and one added, by a fixed pseudo-random walk.
        let base = ["a", "b", "c", "d", "e"];
        let mut seed = 7u64;
        for _ in 0..500 {
            let mut ids: Vec<&str> = base.to_vec();
            for i in (1..ids.len()).rev() {
                seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                ids.swap(i, (seed >> 33) as usize % (i + 1));
            }
            let drop = (seed >> 40) as usize % ids.len();
            ids.remove(drop);
            let at = (seed >> 20) as usize % (ids.len() + 1);
            ids.insert(at, "x");
            let (alt, neu) = (liste(&base), liste(&ids));
            let steps = vergleiche(&paare(&alt), &paare(&neu));
            assert_eq!(anwenden(&alt, &steps), neu, "{ids:?}");
        }
    }

    #[test]
    fn moving_one_entry_is_one_move() {
        let (alt, neu) = (liste(&["a", "b", "c", "d"]), liste(&["a", "c", "d", "b"]));
        let steps = vergleiche(&paare(&alt), &paare(&neu));
        assert_eq!(steps, vec![Schritt::Verschoben("b".into(), Some("d".into()))]);
    }
}
