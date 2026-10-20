//! Backend Standard : `POST /v1/audio/transcriptions` avec `stream=true`.
//!
//! L'audio complet est envoyé en une requête multipart ; le texte revient en
//! Server-Sent Events (`transcript.text.delta` puis `transcript.text.done`).

use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use reqwest::multipart::{Form, Part};
use serde::Deserialize;

use super::sse::SseParser;
use super::{AudioPayload, Backend, BoxFuture, Progress, ProgressFn};
use crate::config::ApiConfig;

pub struct StandardBackend {
    client: reqwest::Client,
    endpoint: String,
    warm_up_url: String,
    api_key: String,
    model: String,
    language: String,
}

impl StandardBackend {
    pub fn new(api: &ApiConfig, api_key: String) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(concat!("Dictia/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(api.timeout_secs.max(1)))
            .tcp_nodelay(true)
            // Garde la connexion préchauffée ouverte pendant une longue dictée.
            .pool_idle_timeout(Duration::from_secs(300))
            .http2_keep_alive_interval(Duration::from_secs(20))
            .http2_keep_alive_while_idle(true)
            .build()
            .context("initialisation du client HTTP")?;
        let base = api.base_url.trim_end_matches('/');
        Ok(Self {
            client,
            endpoint: endpoint(base),
            warm_up_url: format!("{base}/models"),
            api_key,
            model: api.model.clone(),
            language: api.language.clone(),
        })
    }

    async fn run(
        &self,
        audio: AudioPayload,
        prompt: &str,
        progress: &mut ProgressFn<'_>,
    ) -> Result<String> {
        let mut form = Form::new();
        for (name, value) in form_fields(&self.model, &self.language, prompt) {
            form = form.text(name, value);
        }
        let file = Part::bytes(audio.bytes)
            .file_name(audio.file_name)
            .mime_str(audio.mime)?;
        form = form.part("file", file);

        let mut response = self
            .client
            .post(&self.endpoint)
            .bearer_auth(&self.api_key)
            .multipart(form)
            .send()
            .await
            .context("envoi de la requête de transcription")?;
        progress(Progress::ResponseStarted);

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            bail!("{}", api_error_message(status.as_u16(), &body));
        }

        let is_stream = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .is_some_and(|v| v.starts_with("text/event-stream"));
        if !is_stream {
            // Modèles sans streaming (ex. whisper-1) : réponse JSON unique.
            let body = response.text().await?;
            let text = parse_json_text(&body)?;
            progress(Progress::Delta(&text));
            return Ok(text);
        }

        let mut parser = SseParser::default();
        let mut transcript = Transcript::default();
        while let Some(chunk) = response.chunk().await.context("lecture du flux")? {
            for data in parser.push(&chunk) {
                if transcript.handle(&data, progress)? {
                    return Ok(transcript.into_text());
                }
            }
        }
        for data in parser.finish() {
            transcript.handle(&data, progress)?;
        }
        Ok(transcript.into_text())
    }
}

impl Backend for StandardBackend {
    fn name(&self) -> &'static str {
        "standard"
    }

    fn warm_up(&self) -> BoxFuture<'_, ()> {
        Box::pin(async move {
            let start = Instant::now();
            // Une requête légère suffit : DNS, TCP et TLS sont établis et la
            // connexion reste dans le pool pour la transcription.
            match self.client.head(&self.warm_up_url).send().await {
                Ok(r) => log::debug!(
                    "connexion préchauffée en {} ms (HTTP {})",
                    start.elapsed().as_millis(),
                    r.status().as_u16()
                ),
                Err(e) => log::warn!("préchauffage de la connexion impossible : {e}"),
            }
        })
    }

    fn transcribe<'a>(
        &'a self,
        audio: AudioPayload,
        prompt: &'a str,
        progress: &'a mut ProgressFn<'_>,
    ) -> BoxFuture<'a, Result<String>> {
        Box::pin(self.run(audio, prompt, progress))
    }
}

/// URL de l'endpoint de transcription à partir de l'URL de base.
pub fn endpoint(base_url: &str) -> String {
    format!("{}/audio/transcriptions", base_url.trim_end_matches('/'))
}

/// Champs texte de la requête multipart (hors fichier audio).
pub fn form_fields(model: &str, language: &str, prompt: &str) -> Vec<(&'static str, String)> {
    let mut fields = vec![
        ("model", model.to_owned()),
        ("response_format", "json".to_owned()),
        ("stream", "true".to_owned()),
    ];
    if !language.trim().is_empty() {
        fields.push(("language", language.trim().to_owned()));
    }
    if !prompt.trim().is_empty() {
        fields.push(("prompt", prompt.trim().to_owned()));
    }
    fields
}

/// Événement du flux de transcription.
#[derive(Debug, PartialEq, Eq)]
pub enum StreamEvent {
    Delta(String),
    Done(String),
    Error(String),
    Ignored,
}

pub fn parse_event(data: &str) -> Result<StreamEvent> {
    #[derive(Deserialize)]
    struct Raw {
        #[serde(rename = "type", default)]
        kind: String,
        #[serde(default)]
        delta: String,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        error: Option<ApiErrorBody>,
    }

    if data.trim() == "[DONE]" {
        return Ok(StreamEvent::Ignored);
    }
    let raw: Raw =
        serde_json::from_str(data).with_context(|| format!("événement illisible : {data}"))?;
    Ok(match raw.kind.as_str() {
        "transcript.text.delta" => StreamEvent::Delta(raw.delta),
        "transcript.text.done" => StreamEvent::Done(raw.text.unwrap_or_default()),
        "error" => StreamEvent::Error(
            raw.error
                .map(|e| e.message)
                .unwrap_or_else(|| data.to_owned()),
        ),
        _ => StreamEvent::Ignored,
    })
}

/// Assemble le texte à partir des événements du flux.
#[derive(Debug, Default)]
struct Transcript {
    deltas: String,
    done: Option<String>,
}

impl Transcript {
    /// Renvoie `true` quand la transcription est terminée.
    fn handle(&mut self, data: &str, progress: &mut ProgressFn<'_>) -> Result<bool> {
        match parse_event(data)? {
            StreamEvent::Delta(d) => {
                progress(Progress::Delta(&d));
                self.deltas.push_str(&d);
                Ok(false)
            }
            StreamEvent::Done(text) => {
                self.done = Some(text);
                Ok(true)
            }
            StreamEvent::Error(msg) => Err(anyhow!("erreur de l'API : {msg}")),
            StreamEvent::Ignored => Ok(false),
        }
    }

    /// Le texte final fait foi ; sinon, la concaténation des fragments.
    fn into_text(self) -> String {
        self.done.unwrap_or(self.deltas)
    }
}

#[derive(Debug, Deserialize)]
struct ApiErrorBody {
    #[serde(default)]
    message: String,
}

fn parse_json_text(body: &str) -> Result<String> {
    #[derive(Deserialize)]
    struct Body {
        text: String,
    }
    let parsed: Body =
        serde_json::from_str(body).with_context(|| format!("réponse illisible : {body}"))?;
    Ok(parsed.text)
}

/// Message d'erreur lisible à partir d'une réponse HTTP en échec.
pub fn api_error_message(status: u16, body: &str) -> String {
    #[derive(Deserialize)]
    struct Envelope {
        error: ApiErrorBody,
    }
    let detail = serde_json::from_str::<Envelope>(body)
        .map(|e| e.error.message)
        .unwrap_or_else(|_| body.trim().chars().take(300).collect());
    let hint = match status {
        401 => " (clé API invalide ?)",
        429 => " (quota ou limite de débit atteint)",
        _ => "",
    };
    format!("HTTP {status}{hint} : {detail}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_endpoint_from_base_url() {
        assert_eq!(
            endpoint("https://api.openai.com/v1/"),
            "https://api.openai.com/v1/audio/transcriptions"
        );
        assert_eq!(
            endpoint("http://localhost:8080/v1"),
            "http://localhost:8080/v1/audio/transcriptions"
        );
    }

    #[test]
    fn form_fields_for_mvp_request() {
        let fields = form_fields("gpt-4o-transcribe", "fr", " Vocabulaire : Dictia. ");
        assert_eq!(
            fields,
            [
                ("model", "gpt-4o-transcribe".to_owned()),
                ("response_format", "json".to_owned()),
                ("stream", "true".to_owned()),
                ("language", "fr".to_owned()),
                ("prompt", "Vocabulaire : Dictia.".to_owned()),
            ]
        );
    }

    #[test]
    fn form_fields_omit_empty_language_and_prompt() {
        let names: Vec<_> = form_fields("m", " ", "")
            .into_iter()
            .map(|(n, _)| n)
            .collect();
        assert_eq!(names, ["model", "response_format", "stream"]);
    }

    #[test]
    fn parses_stream_events() {
        assert_eq!(
            parse_event(r#"{"type":"transcript.text.delta","delta":"Bon"}"#).unwrap(),
            StreamEvent::Delta("Bon".into())
        );
        assert_eq!(
            parse_event(
                r#"{"type":"transcript.text.done","text":"Bonjour.","usage":{"type":"tokens"}}"#
            )
            .unwrap(),
            StreamEvent::Done("Bonjour.".into())
        );
        assert_eq!(
            parse_event(r#"{"type":"error","error":{"message":"oups"}}"#).unwrap(),
            StreamEvent::Error("oups".into())
        );
        assert_eq!(parse_event("[DONE]").unwrap(), StreamEvent::Ignored);
        assert_eq!(
            parse_event(r#"{"type":"transcript.text.segment"}"#).unwrap(),
            StreamEvent::Ignored
        );
        assert!(parse_event("pas du json").is_err());
    }

    #[test]
    fn full_stream_through_parser_and_transcript() {
        let body = concat!(
            "data: {\"type\":\"transcript.text.delta\",\"delta\":\"Bonjour\"}\n\n",
            "data: {\"type\":\"transcript.text.delta\",\"delta\":\" à tous\"}\n\n",
            "data: {\"type\":\"transcript.text.done\",\"text\":\"Bonjour à tous.\"}\n\n",
        )
        .as_bytes();
        let mut parser = SseParser::default();
        let mut transcript = Transcript::default();
        let mut seen = Vec::new();
        let mut progress = |p: Progress<'_>| {
            if let Progress::Delta(d) = p {
                seen.push(d.to_owned());
            }
        };
        let mut finished = false;
        for chunk in body.chunks(7) {
            for data in parser.push(chunk) {
                finished |= transcript.handle(&data, &mut progress).unwrap();
            }
        }
        assert!(finished);
        assert_eq!(seen, ["Bonjour", " à tous"]);
        assert_eq!(transcript.into_text(), "Bonjour à tous.");
    }

    #[test]
    fn transcript_falls_back_to_deltas() {
        let mut t = Transcript::default();
        let mut noop = |_: Progress<'_>| {};
        t.handle(
            r#"{"type":"transcript.text.delta","delta":"Salut"}"#,
            &mut noop,
        )
        .unwrap();
        assert_eq!(t.into_text(), "Salut");
    }

    #[test]
    fn stream_error_event_fails() {
        let mut t = Transcript::default();
        let mut noop = |_: Progress<'_>| {};
        let err = t
            .handle(
                r#"{"type":"error","error":{"message":"trop long"}}"#,
                &mut noop,
            )
            .unwrap_err();
        assert!(err.to_string().contains("trop long"));
    }

    #[test]
    fn non_stream_json_body() {
        assert_eq!(parse_json_text(r#"{"text":"Salut."}"#).unwrap(), "Salut.");
    }

    #[test]
    fn readable_api_errors() {
        assert_eq!(
            api_error_message(
                401,
                r#"{"error":{"message":"Incorrect API key provided","type":"invalid_request_error"}}"#
            ),
            "HTTP 401 (clé API invalide ?) : Incorrect API key provided"
        );
        assert_eq!(
            api_error_message(502, " Bad gateway "),
            "HTTP 502 : Bad gateway"
        );
    }

    /// Faux serveur HTTP : capture la requête et répond en SSE.
    async fn serve_once(response: &'static str) -> (String, tokio::task::JoinHandle<Vec<u8>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut req = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = sock.read(&mut buf).await.unwrap();
                req.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&req);
                let Some(head_end) = text.find("\r\n\r\n") else {
                    continue;
                };
                let len = text[..head_end].lines().find_map(|l| {
                    l.to_ascii_lowercase()
                        .strip_prefix("content-length:")
                        .map(|v| v.trim().parse::<usize>().unwrap())
                });
                let done = match len {
                    Some(len) => req.len() >= head_end + 4 + len,
                    None => req.ends_with(b"0\r\n\r\n"),
                };
                if done || n == 0 {
                    break;
                }
            }
            sock.write_all(response.as_bytes()).await.unwrap();
            sock.shutdown().await.unwrap();
            req
        });
        (base, handle)
    }

    fn backend(base_url: String) -> StandardBackend {
        let api = ApiConfig {
            base_url,
            ..Default::default()
        };
        StandardBackend::new(&api, "sk-test".into()).unwrap()
    }

    #[tokio::test]
    async fn end_to_end_streaming_request() {
        let (base, server) = serve_once(concat!(
            "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n",
            "data: {\"type\":\"transcript.text.delta\",\"delta\":\"Bonjour\"}\n\n",
            "data: {\"type\":\"transcript.text.done\",\"text\":\"Bonjour.\"}\n\n",
        ))
        .await;
        let backend = backend(base);
        let mut events = Vec::new();
        let mut progress = |p: Progress<'_>| events.push(format!("{p:?}"));
        let text = backend
            .transcribe(
                AudioPayload::flac(b"fLaC-fake".to_vec()),
                "Vocabulaire : Dictia.",
                &mut progress,
            )
            .await
            .unwrap();
        assert_eq!(text, "Bonjour.");
        assert_eq!(events, ["ResponseStarted", "Delta(\"Bonjour\")"]);

        let req = String::from_utf8_lossy(&server.await.unwrap()).into_owned();
        assert!(
            req.starts_with("POST /v1/audio/transcriptions HTTP/1.1"),
            "{req}"
        );
        assert!(req.contains("authorization: Bearer sk-test"), "{req}");
        assert!(req.contains("multipart/form-data; boundary="), "{req}");
        for expected in [
            "name=\"model\"\r\n\r\ngpt-4o-transcribe",
            "name=\"language\"\r\n\r\nfr",
            "name=\"stream\"\r\n\r\ntrue",
            "name=\"prompt\"\r\n\r\nVocabulaire : Dictia.",
            "name=\"file\"; filename=\"audio.flac\"\r\nContent-Type: audio/flac\r\n\r\nfLaC-fake",
        ] {
            assert!(req.contains(expected), "manque {expected:?} dans :\n{req}");
        }
    }

    #[tokio::test]
    async fn end_to_end_http_error() {
        let (base, _server) = serve_once(concat!(
            "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nConnection: close\r\n\r\n",
            "{\"error\":{\"message\":\"Incorrect API key provided\"}}",
        ))
        .await;
        let mut noop = |_: Progress<'_>| {};
        let err = backend(base)
            .transcribe(AudioPayload::flac(vec![0; 8]), "", &mut noop)
            .await
            .unwrap_err();
        assert_eq!(
            err.to_string(),
            "HTTP 401 (clé API invalide ?) : Incorrect API key provided"
        );
    }
}
