# Dictia — cahier des charges

> Document de référence pour le développement de Dictia. Reconstitué à partir des
> spécifications du MVP ; à compléter au fil des étapes.

## Vision

Dictia est une application Windows de dictée vocale écrite en Rust. L'utilisateur
appuie sur un raccourci global, parle, appuie de nouveau : le texte transcrit est
tapé au curseur, dans la fenêtre où il travaillait, comme s'il l'avait saisi au
clavier.

## Priorité absolue : latence minimale

Chaque choix technique est arbitré en faveur du délai le plus court entre la fin
de la parole et l'apparition du texte. En cas de conflit entre deux exigences,
on retient la solution la plus simple qui respecte cette priorité, et on
documente le choix.

Leviers de latence :
- pré-ouverture de la connexion TLS dès le début de l'enregistrement ;
- suppression des silences de début et de fin avant l'envoi ;
- audio mono 16 kHz encodé en FLAC (compression sans perte, upload réduit) ;
- réponse de l'API en streaming ;
- frappe directe via `SendInput` (pas de presse-papier) ;
- mesure systématique de chaque étape.

## Plateforme et contraintes techniques

- Cible : Windows 10/11 x64. Binaire unique `dictia.exe`, sans installeur.
- Langage : Rust stable, édition 2021.
- Code spécifique à Win32 (`SendInput`, `SetForegroundWindow`,
  `GetForegroundWindow`, boucle de messages…) isolé dans `src/platform/windows.rs`
  derrière `#[cfg(windows)]`. La logique métier reste testable sous Linux
  (`cargo test`).
- Profil release optimisé : `lto = true`, `codegen-units = 1`, `opt-level = 3`,
  `strip = true`, `panic = "abort"`.
- Aucun secret dans le code : la clé API est lue dans `config.toml` ou dans la
  variable d'environnement `OPENAI_API_KEY`.

## Architecture (modules)

| Module          | Rôle |
|-----------------|------|
| `config`        | Fichiers TOML dans `%APPDATA%\Dictia\` (création au premier lancement, valeurs par défaut commentées). |
| `hotkey`        | Raccourci global (défaut `Ctrl+Alt+Espace`, mode bascule). |
| `audio`         | Capture micro (cpal), conversion mono 16 kHz, VAD (suppression des silences), encodage FLAC en mémoire. |
| `transcription` | Trait `Backend` ; implémentation **Standard** (`/v1/audio/transcriptions`), puis **Realtime** (WebSocket) plus tard. |
| `vocab`         | Construction du prompt et remplacements post-transcription depuis `vocab.toml`. |
| `inject`        | Frappe du texte au curseur dans la fenêtre mémorisée. |
| `metrics`       | Journalisation des latences de chaque étape. |
| `tray`          | Icône de la zone de notification et menu. |
| `platform`      | Appels Win32 isolés. |

## Fichiers de configuration

`%APPDATA%\Dictia\config.toml` : clé API, modèle, langue, raccourci, réglages
audio (seuil de silence, marge), réglages d'injection.

`%APPDATA%\Dictia\vocab.toml` : termes et contexte injectés dans le prompt du
modèle ; liste de remplacements `de → vers` appliqués au texte transcrit.

## Ordre de développement suggéré

1. **MVP utilisable** : raccourci global → capture micro → API OpenAI (backend
   Standard, `gpt-4o-transcribe`, `language=fr`, `stream=true`) → frappe du texte
   au curseur. Icône de notification avec menu Quitter. Journalisation des
   latences. CI GitHub Actions produisant `dictia.exe`.
2. **Backend Realtime** : streaming audio pendant l'enregistrement (WebSocket
   Realtime API) pour que la transcription soit quasi terminée au relâchement.
3. **Confort** : mode « maintenir pour parler », sons/indicateur visuel
   d'enregistrement, choix du micro, rechargement à chaud de la configuration.
4. **Robustesse** : reprise sur erreur réseau, file d'attente hors ligne,
   gestion fine des fenêtres élevées (UAC), démarrage automatique avec Windows.
5. **Distribution** : signature du binaire, publication de releases, mises à
   jour.

## Règles de travail

- Versions de crates stables et récentes.
- Commits atomiques, messages clairs ; une branche par étape, pull request vers
  `main`.
- `cargo check` et `cargo clippy -- -D warnings` propres pour la cible
  `x86_64-pc-windows-gnu` ; `cargo test` vert sous Linux.
