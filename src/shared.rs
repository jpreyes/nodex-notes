//! Datos que comparten varios equipos a través de la carpeta sincronizada (`.nodex/`).
//!
//! Cada equipo los tiene en memoria y los reescribe enteros al guardar. Para que un equipo no
//! pise lo que agregó otro, antes de guardar (y cuando el archivo cambia por fuera) se junta con
//! lo que hay en disco: se compara el disco con la `base` (cómo estaba el archivo la última vez
//! que este equipo lo leyó o escribió) y se aplica a lo de aquí solo lo que cambió el otro.

/// Aplica a `mine` lo que otro equipo cambió en disco desde `base`: lo que agregó se agrega, lo
/// que quitó se quita, y lo que modificó se toma si aquí no se había tocado. Lo que se cambió
/// aquí y no allá queda como está. Devuelve si `mine` cambió.
pub fn merge_list<T: Clone + PartialEq, K: PartialEq>(mine: &mut Vec<T>, base: &[T], disk: &[T], key: impl Fn(&T) -> K) -> bool {
    let mut changed = false;
    // Lo que el otro quitó.
    for b in base {
        let k = key(b);
        if !disk.iter().any(|d| key(d) == k) {
            let before = mine.len();
            mine.retain(|m| key(m) != k);
            changed |= mine.len() != before;
        }
    }
    for d in disk {
        let k = key(d);
        match base.iter().find(|b| key(b) == k) {
            // Lo que el otro agregó.
            None => {
                if !mine.iter().any(|m| key(m) == k) {
                    mine.push(d.clone());
                    changed = true;
                }
            }
            // Lo que el otro modificó (y aquí sigue como estaba).
            Some(b) if b != d => {
                if let Some(m) = mine.iter_mut().find(|m| key(m) == k) {
                    if *m == *b {
                        *m = d.clone();
                        changed = true;
                    }
                }
            }
            Some(_) => {}
        }
    }
    changed
}

/// Agrega a `mine` lo de `other` que falte (para las copias en conflicto, que no tienen base).
pub fn absorb_list<T: Clone, K: PartialEq>(mine: &mut Vec<T>, other: &[T], key: impl Fn(&T) -> K) -> bool {
    let mut changed = false;
    for o in other {
        let k = key(o);
        if !mine.iter().any(|m| key(m) == k) {
            mine.push(o.clone());
            changed = true;
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn applies_only_what_the_other_changed() {
        let key = |x: &(u32, &str)| x.0;
        // Base común: 1, 2, 3. Aquí: se quitó el 1 y se agregó el 4. El otro: quitó el 2,
        // cambió el 3 y agregó el 5.
        let base = vec![(1, "a"), (2, "b"), (3, "c")];
        let mut mine = vec![(2, "b"), (3, "c"), (4, "mío")];
        let disk = vec![(1, "a"), (3, "C del otro"), (5, "del otro")];
        assert!(merge_list(&mut mine, &base, &disk, key));
        assert_eq!(mine, vec![(3, "C del otro"), (4, "mío"), (5, "del otro")], "el 1 no vuelve, el 2 se va, el 4 queda");
        // Sin cambios del otro: nada que hacer (lo que quité aquí no reaparece).
        let mut mine = vec![(2, "b")];
        assert!(!merge_list(&mut mine, &base, &base, key));
        assert_eq!(mine, vec![(2, "b")]);
        // Los dos cambiaron lo mismo: queda lo de aquí.
        let mut mine = vec![(3, "C mío")];
        merge_list(&mut mine, &base, &disk, key);
        assert!(mine.contains(&(3, "C mío")));
        let mut mine = vec![1, 2];
        assert!(absorb_list(&mut mine, &[2, 3], |x| *x));
        assert_eq!(mine, vec![1, 2, 3]);
    }
}
