//! Lo que la IA entiende de los correos: un resumen, a qué espacio corresponden, los
//! compromisos (tuyos y de otros) con su fecha, las reuniones o visitas, y qué tareas
//! pendientes parece cumplir cada uno ("verificar compromisos").

use crate::agenda::Task;
use crate::mail::Mail;
use serde::Deserialize;

/// Correos por consulta a la IA.
pub const BATCH: usize = 8;

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct Commitment {
    pub quien: String,
    pub que: String,
    pub fecha: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct MailEvent {
    pub titulo: String,
    pub fecha: String,
    pub hora: String,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
#[serde(default)]
pub struct MailResult {
    pub id: String,
    pub resumen: String,
    pub importante: bool,
    pub espacio: String,
    pub compromisos: Vec<Commitment>,
    pub eventos: Vec<MailEvent>,
    /// Claves de tareas pendientes ("t3") que este correo muestra como cumplidas.
    pub cumple: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct Reply {
    correos: Vec<MailResult>,
}

pub fn parse_reply(text: &str) -> Result<Vec<MailResult>, String> {
    let (Some(a), Some(b)) = (text.find('{'), text.rfind('}')) else { return Err("la IA no devolvió JSON".into()) };
    serde_json::from_str::<Reply>(&text[a..=b]).map(|r| r.correos).map_err(|e| format!("JSON inválido de la IA: {e}"))
}

pub fn is_me(who: &str) -> bool {
    matches!(who.trim().to_lowercase().as_str(), "" | "yo" | "me" | "mí" | "mi" | "yo mismo" | "yo misma")
}

fn today_long(today: &str) -> String {
    use chrono::Datelike;
    const DIAS: [&str; 7] = ["lunes", "martes", "miércoles", "jueves", "viernes", "sábado", "domingo"];
    match chrono::NaiveDate::parse_from_str(today, "%Y-%m-%d") {
        Ok(d) => format!("{} {today}", DIAS[d.weekday().num_days_from_monday() as usize]),
        Err(_) => today.to_string(),
    }
}

/// El prompt para un lote de correos (claves "c1", "c2"…) con las tareas pendientes ("t1"…).
pub fn prompt(mails: &[&Mail], pending: &[(String, Task)], workspaces: &[String], facts: &[String], today: &str) -> (String, String) {
    let system = format!(
        r#"Revisas los correos de una persona que escribe en español para no perder compromisos. Devuelve SOLO un objeto JSON válido, sin texto adicional, con esta forma exacta:
{{"correos": [{{"id": "c1", "resumen": "", "importante": false, "espacio": "", "compromisos": [{{"quien": "yo", "que": "", "fecha": ""}}], "eventos": [{{"titulo": "", "fecha": "", "hora": ""}}], "cumple": []}}]}}

Reglas:
- Un objeto por correo, con su id.
- resumen: una frase corta con lo esencial (qué pide, informa o acuerda).
- importante: true si pide algo a la persona, fija una fecha, espera respuesta o cambia un plan.
- espacio: el espacio de trabajo de la lista al que corresponde, escrito igual; "" si no está claro.
- compromisos: acciones concretas pendientes. "quien" = "yo" si la persona debe hacerlo (lo que le piden en un correo recibido, o lo que ella prometió en uno enviado); si no, el nombre de quien se comprometió (por ejemplo, el que escribe "te lo envío el viernes"). "que" con verbo en infinitivo. "fecha" AAAA-MM-DD si se indica o se deduce ("el martes" = la fecha de ese martes), si no "". No incluyas compromisos que ya pasaron o que otro correo del lote muestra resueltos.
- eventos: reuniones, visitas o llamadas con fecha (AAAA-MM-DD) y hora (HH:MM) si se indica.
- cumple: claves de las tareas pendientes (lista de abajo) que este correo muestra como cumplidas (por ejemplo, llega lo que alguien había prometido enviar). Solo si es claro.
- Correos automáticos, boletines, avisos o publicidad: solo resumen, sin compromisos.
Hoy es {}."#,
        today_long(today)
    );
    let mut user = String::new();
    user += &format!("Espacios de trabajo: {}\n", workspaces.join(", "));
    if !facts.is_empty() {
        user += "Lo que la persona ya aclaró:\n";
        for f in facts {
            user += &format!("- {f}\n");
        }
    }
    if !pending.is_empty() {
        user += "\nTareas pendientes:\n";
        for (k, t) in pending {
            user += &format!("[{k}] {}", t.text);
            if let Some(d) = &t.due {
                user += &format!(" (vence {d})");
            }
            user.push('\n');
        }
    }
    user += "\nCorreos:\n";
    for (i, m) in mails.iter().enumerate() {
        let body: String = m.body.chars().take(3000).collect();
        let dir = if m.sent { "ENVIADO por la persona" } else { "RECIBIDO" };
        user += &format!("[c{}] {dir}\nDe: {}\nPara: {}\nFecha: {}\nAsunto: {}\n<<<\n{body}\n>>>\n\n", i + 1, m.from, m.to, m.date, m.subject);
    }
    (system, user)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_and_reply() {
        let m = Mail { from: "María <maria@cic.cl>".into(), to: "jp@gmail.com".into(), subject: "Visita".into(), date: "2026-09-26 08:40".into(), body: "Necesito la cubicación antes del martes.".into(), ..Mail::default() };
        let t = crate::agenda::parse_task("2026-09-24 enviar planos @Juan +Consorcio due:2026-09-30 id:cic01").unwrap();
        let (system, user) = prompt(&[&m], &[("t1".into(), t)], &["Consorcio".into()], &[], "2026-09-26");
        assert!(system.contains("sábado 2026-09-26"));
        assert!(user.contains("[t1] enviar planos @Juan (vence 2026-09-30)\n"), "{user}");
        assert!(user.contains("[c1] RECIBIDO\nDe: María <maria@cic.cl>"), "{user}");
        let r = parse_reply(r#"```json
{"correos": [{"id": "c1", "resumen": "Pide la cubicación", "importante": true, "espacio": "Consorcio",
  "compromisos": [{"quien": "yo", "que": "Enviar la cubicación", "fecha": "2026-09-29"}], "cumple": ["t1"]}]}
```"#)
        .unwrap();
        assert_eq!((r[0].compromisos[0].fecha.as_str(), r[0].cumple[0].as_str()), ("2026-09-29", "t1"));
        assert!(is_me("Yo") && !is_me("Juan"));
    }
}
