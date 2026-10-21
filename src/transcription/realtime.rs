//! Backend Realtime : l'audio est envoyé **pendant** la dictée sur une
//! WebSocket (`/v1/realtime?intent=transcription`).
//!
//! Au relâchement du raccourci, il ne reste qu'à valider le tampon
//! (`input_audio_buffer.commit`) : l'upload est déjà fait et, avec les
//! modèles « live », l'essentiel du texte aussi.

use std::time::Duration;

use anyhow::{anyhow, bail, Context, Result};
use base64::Engine as _;
use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message as WsMessage;

use super::Progress;

/// Fréquence imposée par l'API Realtime.
pub const RATE: u32 = 24_000;

/// Modèles proposés, du plus réactif au plus classique.
pub const MODELS: [&str; 4] = [
    "gpt-live-transcribe",
    "gpt-realtime-whisper",
    "gpt-4o-transcribe",
    "gpt-4o-mini-transcribe",
];

/// Paramètres d'une session.
#[derive(Clone)]
pub struct RealtimeConfig {
    pub url: String,
    pub api_key: String,
    pub model: String,
    pub language: String,
    pub prompt: String,
    pub keywords: Vec<String>,
    pub timeout: Duration,
}

/// URL WebSocket à partir de l'URL de base HTTP de l'API.
pub fn endpoint(base_url: &str) -> String {
    let base = base_url.trim_end_matches('/');
    let ws = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_owned()
    };
    format!("{ws}/realtime?intent=transcription")
}

/// Configuration de la session : transcription seule, audio PCM 24 kHz,
/// sans détection de tour (c'est le raccourci qui décide de la fin).
pub fn session_update(model: &str, language: &str, prompt: &str, keywords: &[String]) -> Value {
    let mut transcription = json!({ "model": model });
    if !language.trim().is_empty() {
        transcription["language"] = json!(language.trim());
    }
    // Chaque famille de modèles a sa façon d'être guidée.
    match model {
        "gpt-live-transcribe" | "gpt-transcribe" => {
            let words: Vec<&str> = keywords
                .iter()
                .map(|k| k.trim())
                .filter(|k| !k.is_empty())
                .collect();
            if !words.is_empty() {
                transcription["keywords"] = json!(words);
            }
        }
        "gpt-realtime-whisper" => transcription["delay"] = json!("minimal"),
        _ => {
            if !prompt.trim().is_empty() {
                transcription["prompt"] = json!(prompt.trim());
            }
        }
    }
    json!({
        "type": "session.update",
        "session": {
            "type": "transcription",
            "audio": {
                "input": {
                    "format": { "type": "audio/pcm", "rate": RATE },
                    "transcription": transcription,
                    "turn_detection": null,
                    "noise_reduction": { "type": "near_field" }
                }
            }
        }
    })
}

/// Échantillons `f32` (-1..1) → PCM 16 bits.
pub fn to_pcm16(samples: &[f32]) -> Vec<i16> {
    samples
        .iter()
        .map(|&x| (x.clamp(-1.0, 1.0) * f32::from(i16::MAX)).round() as i16)
        .collect()
}

/// Événement d'ajout d'audio (PCM 16 bits petit-boutiste, en base64).
pub fn append_event(pcm: &[i16]) -> String {
    let bytes: Vec<u8> = pcm.iter().flat_map(|s| s.to_le_bytes()).collect();
    json!({
        "type": "input_audio_buffer.append",
        "audio": base64::engine::general_purpose::STANDARD.encode(bytes),
    })
    .to_string()
}

pub const COMMIT_EVENT: &str = r#"{"type":"input_audio_buffer.commit"}"#;

/// Événement serveur utile à Dictia.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerEvent {
    Delta(String),
    Committed { item_id: String },
    Completed { item_id: String, transcript: String },
    Failed(String),
    Error(String),
    Other,
}

pub fn parse_event(text: &str) -> Result<ServerEvent> {
    #[derive(Deserialize)]
    struct Detail {
        #[serde(default)]
        message: String,
    }
    #[derive(Deserialize)]
    struct Raw {
        #[serde(rename = "type", default)]
        kind: String,
        #[serde(default)]
        item_id: String,
        #[serde(default)]
        delta: Option<String>,
        #[serde(default)]
        transcript: Option<String>,
        #[serde(default)]
        error: Option<Detail>,
    }
    let raw: Raw =
        serde_json::from_str(text).with_context(|| format!("événement illisible : {text}"))?;
    let message = || {
        raw.error
            .as_ref()
            .map(|e| e.message.clone())
            .unwrap_or_default()
    };
    Ok(match raw.kind.as_str() {
        "conversation.item.input_audio_transcription.delta" => {
            ServerEvent::Delta(raw.delta.clone().unwrap_or_default())
        }
        "input_audio_buffer.committed" => ServerEvent::Committed {
            item_id: raw.item_id.clone(),
        },
        "conversation.item.input_audio_transcription.completed" => ServerEvent::Completed {
            item_id: raw.item_id.clone(),
            transcript: raw.transcript.clone().unwrap_or_default(),
        },
        "conversation.item.input_audio_transcription.failed" => ServerEvent::Failed(message()),
        "error" => ServerEvent::Error(message()),
        _ => ServerEvent::Other,
    })
}

/// Assemble le texte final d'une dictée.
#[derive(Debug, Default)]
pub struct Assembler {
    commit_sent: bool,
    committed: Option<String>,
    deltas: String,
    done: Option<String>,
}

impl Assembler {
    pub fn commit_sent(&mut self) {
        self.commit_sent = true;
    }

    /// Renvoie `true` quand la transcription de la dictée est complète.
    pub fn handle(&mut self, event: &ServerEvent) -> Result<bool> {
        match event {
            ServerEvent::Delta(d) => self.deltas.push_str(d),
            ServerEvent::Committed { item_id } => self.committed = Some(item_id.clone()),
            ServerEvent::Completed {
                item_id,
                transcript,
            } => {
                let ours = self
                    .committed
                    .as_deref()
                    .map_or(self.commit_sent, |c| c == item_id);
                if ours {
                    self.done = Some(transcript.clone());
                    return Ok(true);
                }
            }
            ServerEvent::Failed(msg) => bail!("transcription en direct échouée : {msg}"),
            ServerEvent::Error(msg) => bail!("erreur de l'API Realtime : {msg}"),
            ServerEvent::Other => {}
        }
        Ok(false)
    }

    pub fn into_text(self) -> String {
        self.done.unwrap_or(self.deltas)
    }
}

/// Déroule une session : connexion, configuration, envoi de l'audio reçu
/// sur `audio` jusqu'à sa fermeture (fin de la dictée), validation puis
/// attente du texte complet.
pub async fn run_session(
    config: RealtimeConfig,
    mut audio: mpsc::UnboundedReceiver<Vec<i16>>,
    mut progress: impl FnMut(Progress<'_>) + Send,
) -> Result<String> {
    let mut request = config
        .url
        .as_str()
        .into_client_request()
        .context("URL Realtime invalide")?;
    request.headers_mut().insert(
        "Authorization",
        HeaderValue::from_str(&format!("Bearer {}", config.api_key)).context("clé API invalide")?,
    );
    let (ws, _) = tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async(request),
    )
    .await
    .context("connexion Realtime trop lente")?
    .context("connexion Realtime impossible")?;
    let (mut sink, mut stream) = ws.split();
    let update = session_update(
        &config.model,
        &config.language,
        &config.prompt,
        &config.keywords,
    );
    sink.send(WsMessage::text(update.to_string())).await?;

    let mut assembler = Assembler::default();
    let mut sent_samples = 0usize;
    // Pendant la dictée : envoi de l'audio au fil de l'eau, tout en lisant
    // les premiers fragments de texte.
    loop {
        tokio::select! {
            chunk = audio.recv() => match chunk {
                Some(pcm) => {
                    sent_samples += pcm.len();
                    sink.send(WsMessage::text(append_event(&pcm))).await?;
                }
                None => break,
            },
            msg = stream.next() => match msg {
                Some(msg) => {
                    handle_message(msg?, &mut assembler, &mut progress)?;
                }
                None => bail!("connexion Realtime interrompue"),
            },
        }
    }

    // L'API refuse un tampon de moins de 100 ms.
    if sent_samples < (RATE / 10) as usize {
        let _ = sink.close().await;
        return Ok(String::new());
    }
    sink.send(WsMessage::text(COMMIT_EVENT)).await?;
    assembler.commit_sent();
    progress(Progress::ResponseStarted);

    let finish = async {
        while let Some(msg) = stream.next().await {
            if handle_message(msg?, &mut assembler, &mut progress)? {
                return Ok(());
            }
        }
        bail!("connexion Realtime fermée avant la fin de la transcription")
    };
    tokio::time::timeout(config.timeout, finish)
        .await
        .context("transcription en direct trop lente")??;
    let _ = sink.close().await;
    Ok(assembler.into_text())
}

/// Traite un message du serveur ; `true` quand le texte est complet.
fn handle_message(
    msg: WsMessage,
    assembler: &mut Assembler,
    progress: &mut impl FnMut(Progress<'_>),
) -> Result<bool> {
    match msg {
        WsMessage::Text(text) => {
            let event = parse_event(&text)?;
            if let ServerEvent::Delta(d) = &event {
                progress(Progress::Delta(d));
            }
            assembler.handle(&event)
        }
        WsMessage::Close(frame) => Err(anyhow!(
            "connexion Realtime fermée{}",
            frame
                .map(|f| format!(" : {}", f.reason))
                .unwrap_or_default()
        )),
        _ => Ok(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_websocket_endpoint() {
        assert_eq!(
            endpoint("https://api.openai.com/v1/"),
            "wss://api.openai.com/v1/realtime?intent=transcription"
        );
        assert_eq!(
            endpoint("http://127.0.0.1:9000/v1"),
            "ws://127.0.0.1:9000/v1/realtime?intent=transcription"
        );
    }

    #[test]
    fn session_update_for_live_model_uses_keywords() {
        let v = session_update(
            "gpt-live-transcribe",
            "fr",
            "Contexte.",
            &["Dictia".into(), " ".into(), "n8n".into()],
        );
        assert_eq!(v["type"], "session.update");
        assert_eq!(v["session"]["type"], "transcription");
        let input = &v["session"]["audio"]["input"];
        assert_eq!(input["format"], json!({"type": "audio/pcm", "rate": 24000}));
        assert!(input["turn_detection"].is_null());
        assert_eq!(
            input["transcription"],
            json!({"model": "gpt-live-transcribe", "language": "fr", "keywords": ["Dictia", "n8n"]})
        );
    }

    #[test]
    fn session_update_per_model_family() {
        let whisper = session_update("gpt-realtime-whisper", "fr", "Contexte.", &["x".into()]);
        assert_eq!(
            whisper["session"]["audio"]["input"]["transcription"],
            json!({"model": "gpt-realtime-whisper", "language": "fr", "delay": "minimal"})
        );
        let classic = session_update("gpt-4o-transcribe", "", " Contexte. ", &["x".into()]);
        assert_eq!(
            classic["session"]["audio"]["input"]["transcription"],
            json!({"model": "gpt-4o-transcribe", "prompt": "Contexte."})
        );
    }

    #[test]
    fn audio_is_base64_pcm16_little_endian() {
        assert_eq!(to_pcm16(&[0.0, 1.0, -1.0, 2.0]), [0, 32767, -32767, 32767]);
        let event: Value = serde_json::from_str(&append_event(&[1, -2])).unwrap();
        assert_eq!(event["type"], "input_audio_buffer.append");
        // 01 00 FE FF
        assert_eq!(event["audio"], "AQD+/w==");
    }

    #[test]
    fn parses_server_events() {
        assert_eq!(
            parse_event(r#"{"type":"conversation.item.input_audio_transcription.delta","item_id":"i1","delta":"Bon"}"#).unwrap(),
            ServerEvent::Delta("Bon".into())
        );
        assert_eq!(
            parse_event(r#"{"type":"input_audio_buffer.committed","item_id":"i1","event_id":"e"}"#)
                .unwrap(),
            ServerEvent::Committed {
                item_id: "i1".into()
            }
        );
        assert_eq!(
            parse_event(r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"i1","content_index":0,"transcript":"Bonjour.","usage":{"type":"duration","seconds":1.2}}"#).unwrap(),
            ServerEvent::Completed { item_id: "i1".into(), transcript: "Bonjour.".into() }
        );
        assert_eq!(
            parse_event(r#"{"type":"error","error":{"type":"invalid_request_error","message":"Model not found"}}"#).unwrap(),
            ServerEvent::Error("Model not found".into())
        );
        assert_eq!(
            parse_event(r#"{"type":"session.updated","session":{}}"#).unwrap(),
            ServerEvent::Other
        );
        assert!(parse_event("pas du json").is_err());
    }

    #[test]
    fn assembler_waits_for_the_committed_item() {
        let mut a = Assembler::default();
        assert!(!a.handle(&ServerEvent::Delta("Bon".into())).unwrap());
        a.commit_sent();
        assert!(!a
            .handle(&ServerEvent::Committed {
                item_id: "i2".into()
            })
            .unwrap());
        // Un autre élément (ancien) ne termine pas la dictée.
        assert!(!a
            .handle(&ServerEvent::Completed {
                item_id: "i1".into(),
                transcript: "x".into()
            })
            .unwrap());
        assert!(a
            .handle(&ServerEvent::Completed {
                item_id: "i2".into(),
                transcript: "Bonjour.".into()
            })
            .unwrap());
        assert_eq!(a.into_text(), "Bonjour.");
    }

    #[test]
    fn assembler_reports_errors() {
        let mut a = Assembler::default();
        assert!(a.handle(&ServerEvent::Error("quota".into())).is_err());
        assert!(a.handle(&ServerEvent::Failed("audio".into())).is_err());
    }

    /// Faux serveur Realtime : vérifie le protocole et répond comme l'API.
    #[tokio::test]
    // Signature du rappel imposée par tungstenite.
    #[allow(clippy::result_large_err)]
    async fn end_to_end_session_against_fake_server() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut auth = String::new();
            let mut ws = tokio_tungstenite::accept_hdr_async(
                tcp,
                |req: &tokio_tungstenite::tungstenite::handshake::server::Request, resp| {
                    auth = req.headers()["authorization"].to_str().unwrap().to_owned();
                    assert_eq!(req.uri().query(), Some("intent=transcription"));
                    Ok(resp)
                },
            )
            .await
            .unwrap();
            let mut received = Vec::new();
            while let Some(Ok(msg)) = ws.next().await {
                let WsMessage::Text(text) = msg else { continue };
                let v: Value = serde_json::from_str(&text).unwrap();
                received.push(v["type"].as_str().unwrap().to_owned());
                match v["type"].as_str().unwrap() {
                    "input_audio_buffer.append" => {
                        // Transcription « live » pendant la dictée.
                        if received
                            .iter()
                            .filter(|t| *t == "input_audio_buffer.append")
                            .count()
                            == 1
                        {
                            ws.send(WsMessage::text(r#"{"type":"conversation.item.input_audio_transcription.delta","item_id":"i1","delta":"Bonjour"}"#)).await.unwrap();
                        }
                    }
                    "input_audio_buffer.commit" => {
                        for reply in [
                            r#"{"type":"input_audio_buffer.committed","item_id":"i1"}"#,
                            r#"{"type":"conversation.item.input_audio_transcription.delta","item_id":"i1","delta":" à tous."}"#,
                            r#"{"type":"conversation.item.input_audio_transcription.completed","item_id":"i1","content_index":0,"transcript":"Bonjour à tous.","usage":{"type":"duration","seconds":1}}"#,
                        ] {
                            ws.send(WsMessage::text(reply)).await.unwrap();
                        }
                    }
                    _ => {}
                }
            }
            (auth, received)
        });

        let (tx, rx) = mpsc::unbounded_channel();
        let config = RealtimeConfig {
            url: endpoint(&format!("http://{addr}/v1")),
            api_key: "sk-test".into(),
            model: "gpt-live-transcribe".into(),
            language: "fr".into(),
            prompt: String::new(),
            keywords: vec!["Dictia".into()],
            timeout: Duration::from_secs(5),
        };
        let mut deltas = Vec::new();
        let session = tokio::spawn(async move {
            let text = run_session(config, rx, |p| {
                if let Progress::Delta(d) = p {
                    deltas.push(d.to_owned());
                }
            })
            .await;
            (text, deltas)
        });
        for _ in 0..3 {
            tx.send(vec![0i16; 2_400]).unwrap(); // 3 × 100 ms
        }
        drop(tx); // fin de la dictée
        let (text, deltas) = session.await.unwrap();
        assert_eq!(text.unwrap(), "Bonjour à tous.");
        assert_eq!(deltas, ["Bonjour", " à tous."]);

        let (auth, received) = server.await.unwrap();
        assert_eq!(auth, "Bearer sk-test");
        assert_eq!(
            received,
            [
                "session.update",
                "input_audio_buffer.append",
                "input_audio_buffer.append",
                "input_audio_buffer.append",
                "input_audio_buffer.commit"
            ]
        );
    }

    #[tokio::test]
    async fn too_short_dictation_is_not_committed() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut ws = tokio_tungstenite::accept_async(tcp).await.unwrap();
            while let Some(Ok(msg)) = ws.next().await {
                if let WsMessage::Text(t) = msg {
                    assert!(!t.contains("commit"), "commit inattendu");
                }
            }
        });
        let (tx, rx) = mpsc::unbounded_channel();
        tx.send(vec![0i16; 480]).unwrap(); // 20 ms
        drop(tx);
        let config = RealtimeConfig {
            url: endpoint(&format!("http://{addr}/v1")),
            api_key: "k".into(),
            model: "gpt-4o-transcribe".into(),
            language: "fr".into(),
            prompt: String::new(),
            keywords: vec![],
            timeout: Duration::from_secs(5),
        };
        assert_eq!(run_session(config, rx, |_| {}).await.unwrap(), "");
    }
}
