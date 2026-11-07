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

## Idées pour plus tard

Pistes notées, non planifiées.

- **Apprendre des corrections** : après une dictée, relire le champ où le
  texte a été tapé (UI Automation sous Windows, service d'accessibilité sous
  Android), le comparer mot à mot au texte tapé et, pour une petite
  correction (un ou deux mots), proposer de l'ajouter au vocabulaire ou aux
  remplacements — toujours avec validation, jamais automatiquement. Limites :
  toutes les applications n'exposent pas leur texte ; une reformulation n'est
  pas une erreur de dictée ; ne relire que le champ visé, juste après la
  dictée. Variante plus simple et fiable : corriger la dictée dans l'onglet
  Historique et en déduire le terme.
- **Extension Chrome pour Gmail (PC)** : un bouton Dictia dans la fenêtre
  « Répondre » de Gmail. L'extension lit l'e-mail ouvert (expéditeur, objet,
  fil), enregistre la voix dans le navigateur, puis l'IA de transformation
  rédige la réponse à partir de l'e-mail et de la consigne dictée (ex. « dis-lui
  oui pour mardi, rappelle le devis »). Le brouillon est écrit directement dans
  Gmail. Installation pour usage personnel, hors Chrome Web Store au début.
- **Android** : pas de module Google Workspace. Dans l'application Gmail, on
  garde la bulle Dictia actuelle pour dicter les réponses.
- **Contexte client depuis le coffre Obsidian** : le coffre est fait de
  fichiers Markdown hébergés sur GitHub. Accès en lecture seule à ce dépôt.
  Convention : chaque fiche client indique l'adresse e-mail du client dans son
  en-tête, pour retrouver la fiche de l'expéditeur. Le serveur personnel garde
  une copie à jour du coffre pour répondre vite. Seule la fiche du client
  concerné est ajoutée à la consigne envoyée à l'IA.
- **Compte et synchronisation sur le serveur personnel** : connexion avec le
  compte Google (OAuth). Le serveur synchronise le vocabulaire, les
  remplacements, les consignes personnalisées, les langues et les réglages
  entre PC, Android et l'extension Chrome.
- **Clés API et appels IA** : les clés API restent sur chaque appareil. Les
  appels à l'IA partent directement de l'appareil vers OpenAI ou Mistral,
  jamais via le serveur, pour ne pas ajouter de délai (priorité latence).
- **Vigilance** : le contenu des e-mails et les fiches clients sont envoyés au
  fournisseur d'IA choisi. L'accès au serveur doit être protégé (HTTPS, jetons
  d'accès).
- **Ordre suggéré** : 1) compte et synchronisation ; 2) extension Chrome
  Gmail ; 3) lien avec le coffre clients.
- **Dicter avec un bouton de souris (PC)** : pour les souris non
  programmables. Surveillance des boutons par un hook souris bas niveau
  (`WH_MOUSE_LL`, dans `src/platform/windows.rs`), rappel minimal pour ne pas
  dépasser le délai de Windows. Choix dans l'onglet Raccourcis : Aucun,
  bouton latéral arrière, bouton latéral avant (recommandés : présents sur
  presque toutes les souris, l'appui est « avalé » pour que le navigateur ne
  revienne pas en arrière), ou molette en appui long. Pour la molette : le
  micro démarre dès l'appui (aucun délai ajouté) ; si le bouton est relâché
  vite, l'enregistrement est annulé et le clic du milieu est renvoyé
  normalement (le défilement automatique n'est plus possible). Au choix :
  maintenir pour parler ou bascule ; détection du bouton par « appuyez sur le
  bouton à utiliser ». Suit le mode choisi (Brut, Traduire, Réécrire…) comme
  le raccourci principal. Limites : pas dans les fenêtres administrateur si
  Dictia ne l'est pas ; certains jeux bloquent ce hook.
