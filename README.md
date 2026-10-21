# Dictia

Dictée vocale pour Windows, écrite en Rust. Appuyez sur **Ctrl+Alt+Espace**,
parlez, appuyez de nouveau (ou maintenez **Alt+Maj+Espace** le temps de
parler) : le texte transcrit par OpenAI est tapé au curseur, dans la fenêtre où
vous étiez.

Objectif n° 1 : **la latence minimale** entre la fin de la parole et
l'apparition du texte. Le cahier des charges complet se trouve dans
[`CLAUDE.md`](CLAUDE.md).

## Statut : MVP (étape 1)

- deux raccourcis globaux : mode bascule et mode « maintenir pour parler » ;
- capture du micro par défaut, conversion mono 16 kHz pendant la capture ;
- suppression des silences de début et de fin, encodage FLAC en mémoire ;
- connexion TLS ouverte dès le 1er appui ;
- transcription `gpt-4o-transcribe` en streaming (`/v1/audio/transcriptions`) ;
- remplacements personnalisés, puis frappe via `SendInput` (sans presse-papier) ;
- icône dans la zone de notification (bleu : prêt, rouge : enregistrement,
  orange : transcription, gris : erreur) ;
- journal des latences de chaque dictée ;
- animation à côté du curseur pendant l'écoute et la transcription ;
- fenêtre Dictia : historique, vocabulaire, raccourcis et réglages,
  appliqués immédiatement sans relancer.

## Installation

Téléchargez `dictia.exe` depuis l'onglet **Actions** du dépôt (artefact
`dictia-<commit>` du dernier build), ou compilez-le vous-même. Aucun
installeur : lancez l'exécutable. Pour le démarrer avec Windows, activez
**Réglages → Lancer au démarrage**.

La fenêtre utilise Microsoft Edge WebView2, déjà présent sur Windows 10 et 11
à jour. L'exécutable de la CI (MSVC) l'embarque entièrement ; une compilation
croisée `x86_64-pc-windows-gnu` a besoin du fichier `WebView2Loader.dll`
(fourni par la crate `webview2-com-sys`, dossier `x64`) à côté de `dictia.exe`.

## Compilation

Prérequis : [Rust stable](https://rustup.rs).

```powershell
# Sous Windows
cargo build --release
# → target\release\dictia.exe
```

Depuis Linux (vérification ou compilation croisée) :

```bash
rustup target add x86_64-pc-windows-gnu
sudo apt install gcc-mingw-w64-x86-64
cargo check  --target x86_64-pc-windows-gnu
cargo clippy --target x86_64-pc-windows-gnu -- -D warnings
cargo build  --release --target x86_64-pc-windows-gnu
```

L'icône de l'exécutable (`assets/dictia.ico`) est intégrée par `build.rs`.
Pour la redessiner : `python3 assets/generate_icon.py` (Pillow requis).

Tests (logique métier, sous Linux comme sous Windows) :

```bash
cargo test
```

La CI GitHub Actions (`.github/workflows/build.yml`) exécute rustfmt, clippy,
les tests et `cargo build --release` sur `windows-latest` à chaque push et
pull request, puis publie `dictia.exe` en artefact.

## Configuration

Le plus simple est la fenêtre de Dictia (clic sur l'icône) : tout ce qui s'y
règle est enregistré dans les fichiers ci-dessous, en conservant leurs
commentaires, et appliqué immédiatement.

Au premier lancement, Dictia crée deux fichiers commentés dans
`%APPDATA%\Dictia\` (Réglages → **Ouvrir le dossier de configuration**).
Après une modification à la main, relancez Dictia.

### `config.toml`

```toml
[api]
api_key = ""                          # vide → variable OPENAI_API_KEY
base_url = "https://api.openai.com/v1"
engine = "standard"                   # ou "realtime" : envoi pendant la dictée
model = "gpt-4o-transcribe"           # modèle du moteur standard
realtime_model = "gpt-live-transcribe" # modèle du moteur realtime
language = "fr"
timeout_secs = 30

[hotkey]
shortcut = "Ctrl+Alt+Espace"          # bascule : Ctrl, Alt, Maj, Win + touche
push_to_talk = "Alt+Maj+Espace"       # maintenir pour parler ; "" = désactivé

[audio]
silence_threshold_db = -50.0          # seuil de silence (dBFS)
padding_ms = 150                      # marge gardée autour de la parole
min_speech_ms = 200                   # en dessous : rien n'est envoyé
release_tail_ms = 300                 # écoute prolongée après relâchement (0 = non)

[inject]
trailing_space = true                 # espace après le texte tapé

[indicator]
enabled = true                        # animation à côté du curseur
```

**Clé API** : renseignez `api_key`, ou laissez-le vide et définissez la
variable d'environnement utilisateur `OPENAI_API_KEY`. La valeur du fichier est
prioritaire. La clé n'est jamais écrite dans le journal.

### `vocab.toml`

```toml
# Transmis au modèle comme prompt : style et ponctuation attendus.
context = "Dictée en français, avec une ponctuation soignée."
# Termes à bien orthographier (ajoutés au prompt).
terms = ["Dictia", "OpenAI"]

# Remplacements appliqués au texte transcrit, dans l'ordre.
[[replacements]]
from = "point virgule"
to = ";"
# whole_word = true      (défaut : mots entiers uniquement)
# case_sensitive = false (défaut : insensible à la casse)

[[replacements]]
from = "à la ligne"
to = "\n"                # tapé comme la touche Entrée
```

### Autres fichiers

| Fichier         | Contenu |
|-----------------|---------|
| `dictia.log`    | Journal (démarrage, erreurs, latences). `DICTIA_LOG=debug` pour plus de détails. |
| `latences.csv`  | Une ligne par dictée : encodage, upload, premier token, fin, frappe, total. |
| `historique.jsonl` | Historique des dictées (texte, heure, application, durée, mode). |
| `webview\`     | Données de la fenêtre (WebView2). |

La variable `DICTIA_CONFIG_DIR` permet d'utiliser un autre dossier de
configuration.

## Utilisation

1. Lancez `dictia.exe` : une pastille bleue avec un micro apparaît dans la zone
   de notification. Un clic dessus ouvre la fenêtre Dictia ; relancer
   `dictia.exe` alors qu'il tourne déjà l'ouvre aussi. Sans clé API, la
   fenêtre s'ouvre directement sur les Réglages.
2. Placez le curseur là où le texte doit aller.
3. Deux façons de dicter :
   - **mode bascule** : **Ctrl+Alt+Espace** (l'icône devient rouge), parlez,
     puis **Ctrl+Alt+Espace** de nouveau ;
   - **maintenir pour parler** : gardez **Alt+Maj+Espace** enfoncé pendant que
     vous parlez, relâchez quand vous avez fini. L'écoute continue 300 ms
     après le relâchement (réglable dans Réglages → *Prolonger après
     relâchement*) pour ne pas couper la fin de la phrase ; ré-appuyer pendant
     ce délai prolonge la même dictée.
4. L'icône devient orange, puis le texte est tapé dans la fenêtre d'origine,
   même si vous en avez changé entre-temps.

Une petite pastille suit le curseur de texte (ou, à défaut, la souris) pendant
la dictée :
- **à l'écoute** : un point rouge qui pulse et des barres qui bougent avec
  votre voix ; si les barres restent plates, le micro ne vous entend pas ;
- **transcription en cours** : trois points orange qui ondulent.

Elle disparaît dès que le texte est tapé. Pour la masquer :
`[indicator] enabled = false`.

### Fournisseurs : OpenAI, Mistral, Local

Réglages → **Fournisseur** choisit le service de transcription ; chacun a
sa clé (Réglages → **Clés API**), son modèle et son adresse :

| Fournisseur | Section de `config.toml` | Clé de secours | Format |
|-------------|--------------------------|----------------|--------|
| OpenAI      | `[api]`                  | `OPENAI_API_KEY` | `prompt`, moteur Realtime disponible |
| Mistral     | `[mistral]`              | `MISTRAL_API_KEY` | Voxtral : termes du Vocabulaire envoyés en `context_bias` |
| Local       | `[local]`                | (facultative) | serveur compatible OpenAI sur votre PC (ex. vLLM) |

Le tableau des Performances distingue les fournisseurs, pour les comparer.

### Moteur standard ou Realtime

- **Standard** (par défaut) : l'audio est envoyé quand vous avez fini de
  parler (FLAC, silences retirés), puis le texte revient en streaming.
- **Realtime** (Réglages → Moteur) : l'audio part **pendant** que vous parlez,
  sur une WebSocket (`/v1/realtime?intent=transcription`, PCM 24 kHz). Au
  relâchement, il ne reste qu'à valider : le texte arrive presque aussitôt.
  Avec `gpt-live-transcribe`, vos termes du Vocabulaire sont transmis comme
  mots-clés ; avec `gpt-4o-transcribe`, comme prompt. Si la session en direct
  échoue (réseau, modèle indisponible), la dictée est automatiquement
  retranscrite par le moteur standard : rien n'est perdu.
- **Écrire pendant la dictée** (Realtime, activé par défaut) : le texte est
  tapé mot par mot au fil de la transcription, dès qu'aucune touche Ctrl, Alt
  ou Win n'est enfoncée (sinon Windows prendrait les lettres pour des
  raccourcis). Le texte final, remplacements compris, corrige au besoin ce qui
  a été tapé. Astuce : un raccourci « maintenir pour parler » sans Ctrl/Alt/Win
  (par exemple **F9**) permet de voir le texte s'écrire pendant que vous
  parlez.

Réglages → **Performances** compare les moteurs : médiane, moyenne et 90e
centile de l'attente (fin de l'enregistrement → texte tapé), calculés à partir
de `latences.csv`.

### La fenêtre Dictia

- **Historique** : vos dictées, avec recherche, filtres, bouton *Copier* et
  bouton *Réinsérer au curseur* (la fenêtre se retire et le texte est tapé
  dans l'application précédente).
- **Vocabulaire** : termes envoyés au modèle et remplacements appliqués au
  texte.
- **Raccourcis** : cliquez sur un raccourci et tapez la nouvelle combinaison ;
  le mode « maintenir pour parler » peut être désactivé.
- **Réglages** : modèle, clé API, animation, espace après le texte, lancement
  au démarrage, couleur d'accent.

Fermer la fenêtre la libère entièrement : Dictia continue de tourner dans la
zone de notification.

En cas d'erreur (réseau, clé invalide, micro), un son système retentit,
l'icône devient grise et le détail est écrit dans `dictia.log`.

### Limites connues

- Windows interdit d'injecter des frappes dans une fenêtre lancée en
  administrateur depuis un programme qui ne l'est pas : lancez Dictia en
  administrateur si nécessaire.
- Le micro utilisé est le périphérique d'entrée par défaut de Windows.
