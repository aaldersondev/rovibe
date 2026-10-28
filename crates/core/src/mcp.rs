//! MCP server over Streamable HTTP. Every tool call is a single POST answered
//! with plain JSON; Studio is reached through the already-open WebSocket, so
//! a call costs one local round trip and no polling.

use std::{sync::Arc, time::Duration};

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::{json, Value};

use base64::Engine;

use crate::{
    agents, assets, input, lint, screenshot,
    state::{Approval, Project, Shared},
    studio, sync,
};

const DEFAULT_PROTOCOL: &str = "2025-06-18";
const INSTRUCTIONS: &str = "Pilote Roblox Studio pour le projet courant. Le code du jeu se modifie dans les fichiers du projet (synchronisés vers Studio), pas via ces outils : ils servent à inspecter la place, exécuter du Luau, lire la console et lancer des playtests. Si ton dossier de travail est une branche à part (`git branch --show-current` commence par `rovibe/`), Studio montre le dossier principal du projet et non le tien : appelle `sync_connect` pour y faire apparaître ta branche avant de tester, et commite ton travail, que l'utilisateur fusionnera depuis l'app. Pour vérifier une fonctionnalité, préfère `playtest_check` (un test complet en un appel, avec verdict) aux appels séparés. Un seul agent à la fois pilote Studio (tests, captures, entrées de jeu) : si on te répond qu'un autre pilote, appelle `studio_wait` au lieu de réessayer.";

fn tools() -> Value {
    let context = json!({
        "type": "string",
        "enum": ["edit", "server", "client"],
        "description": "DataModel visé. `server` et `client` n'existent que pendant un playtest. Défaut : edit."
    });
    let frame = json!({
        "type": "object",
        "description": "Où placer la caméra pour cette image. `target` (chemin d'un Model, d'une BasePart ou d'un Attachment, cadré d'après sa taille) ou `look_at` ([x, y, z]). La caméra se place à `azimuth` degrés autour (0 = de face depuis +Z, 45 = trois quarts, 90 = depuis la droite) et `elevation` degrés au-dessus (0 = à niveau, 90 = vue de dessus), à une distance déduite de la taille sauf `distance` ; `position` [x, y, z] la place directement. `fov` (1 à 120) change le champ de vision pour cette image seulement.",
        "properties": {
            "target": { "type": "string" },
            "look_at": { "type": "array", "items": { "type": "number" } },
            "position": { "type": "array", "items": { "type": "number" } },
            "azimuth": { "type": "number", "description": "Défaut 45." },
            "elevation": { "type": "number", "description": "Défaut 25." },
            "distance": { "type": "number" },
            "fov": { "type": "number" }
        }
    });

    json!([
        {
            "name": "studio_status",
            "description": "Liste les Roblox Studio connectés (place, contexte) et l'état de la synchro du projet.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "run_luau",
            "description": "Exécute du Luau dans Studio. Renvoie les print/warn du code et ses valeurs de retour. En contexte edit, la modification est annulable avec Ctrl+Z.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "code": { "type": "string" },
                    "context": context,
                    "timeout": { "type": "number", "description": "Secondes, défaut 30." }
                },
                "required": ["code"]
            }
        },
        {
            "name": "get_tree",
            "description": "Arborescence de la place en texte indenté : `Nom [Classe]`.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Ex. `game.Workspace.Map`. Défaut : les services principaux." },
                    "depth": { "type": "integer", "description": "Défaut 2." },
                    "max": { "type": "integer", "description": "Nombre maximal d'instances, défaut 300." },
                    "context": context
                }
            }
        },
        {
            "name": "search",
            "description": "Cherche des instances par nom (sous-chaîne, insensible à la casse) et/ou par classe (IsA).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string" },
                    "class": { "type": "string" },
                    "root": { "type": "string" },
                    "limit": { "type": "integer", "description": "Défaut 50." },
                    "context": context
                }
            }
        },
        {
            "name": "get_instance",
            "description": "Détail d'une instance : classe, propriétés courantes, attributs, tags, enfants. `source: true` ajoute le code d'un script.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string" },
                    "source": { "type": "boolean" },
                    "context": context
                },
                "required": ["path"]
            }
        },
        {
            "name": "get_console",
            "description": "Sortie de la console Studio (edit, et serveur et client pendant un test), déjà reçue par l'app : réponse immédiate. Ne renvoie que les lignes que tu n'as pas encore lues, et dit combien il y en a en tout ; « aucune nouvelle ligne » est dit en toutes lettres, jamais par une réponse vide. Passe `recent` pour relire les dernières lignes même déjà lues.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "recent": { "type": "integer", "description": "Relit les N dernières lignes, lues ou non." },
                    "since": { "type": "integer", "description": "Lit à partir d'un curseur précis (renvoyé à chaque appel)." },
                    "limit": { "type": "integer", "description": "Nombre maximal de lignes, défaut 100, les plus récentes." },
                    "level": { "type": "string", "enum": ["output", "info", "warn", "error"] },
                    "context": { "type": "string", "enum": ["edit", "server", "client"], "description": "Seulement les lignes de ce DataModel." },
                    "contains": { "type": "string" }
                }
            }
        },
        {
            "name": "playtest",
            "description": "Démarre ou arrête un test dans Studio. Au démarrage, attend que le serveur (et le client en mode play) soient prêts avant de répondre.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "action": { "type": "string", "enum": ["start", "stop"] },
                    "mode": { "type": "string", "enum": ["play", "run"], "description": "play = avec un joueur (défaut), run = serveur seul." }
                },
                "required": ["action"]
            }
        },
        {
            "name": "check_code",
            "description": "Vérifie le code du projet sans lancer le jeu. Projet TypeScript : erreurs de compilation roblox-ts (types de l'API Roblox, constructions refusées), plus selene sur le Luau écrit à la main. Projet Luau : selene (noms indéfinis, erreurs de syntaxe, variables inutilisées) et luau-lsp (erreurs de type, avec l'API Roblox et l'arborescence du projet). À lancer après une série de modifications, avant un playtest.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Fichiers ou dossiers relatifs au projet. Défaut : `src`." },
                    "warnings": { "type": "boolean", "description": "Lister aussi les avertissements. Par défaut seules les erreurs sont détaillées, les avertissements sont comptés par type." }
                }
            }
        },
        {
            "name": "publish",
            "description": "Publie la place ouverte dans Studio sur Roblox, c'est-à-dire la met en ligne pour les joueurs. L'utilisateur doit d'abord accepter la demande dans l'app : n'appelle cet outil que s'il te l'a demandé.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "agents_status",
            "description": "Montre les autres agents qui travaillent sur ce projet : ce que chacun fait, les fichiers qu'il a réservés ou modifiés récemment. À consulter avant de commencer une tâche et quand une modification est refusée.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "claim_files",
            "description": "Réserve des fichiers ou des dossiers du projet avant d'y travailler, pour que les autres agents n'y touchent pas. Tout ou rien : si une partie est déjà prise, rien n'est réservé. Les fichiers que tu modifies sont de toute façon réservés pour toi pendant 10 minutes.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "paths": { "type": "array", "items": { "type": "string" }, "description": "Chemins relatifs au projet, ex. `src/server/Shop` ou `src/shared/Config.luau`." },
                    "note": { "type": "string", "description": "Ce que tu y fais, visible par les autres agents." }
                },
                "required": ["paths"]
            }
        },
        {
            "name": "release_files",
            "description": "Libère tes réservations quand ta tâche est finie. Sans `paths`, libère tout.",
            "inputSchema": {
                "type": "object",
                "properties": { "paths": { "type": "array", "items": { "type": "string" } } }
            }
        },
        {
            "name": "play_move",
            "description": "Pendant un test Play, fait marcher le personnage du joueur jusqu'à une position ou une instance, avec pathfinding et sauts. Ne prend pas le focus de la fenêtre.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "to": { "description": "`[x, y, z]` ou le chemin d'une instance, ex. `game.Workspace.Checkpoint1`." },
                    "timeout": { "type": "number", "description": "Secondes, défaut 20." }
                },
                "required": ["to"]
            }
        },
        {
            "name": "play_input",
            "description": "Pendant un test Play, envoie des entrées clavier et souris au jeu : touches, clics dans le viewport, clics sur un élément d'interface, texte. Passe par VirtualInput, les entrées du moteur Roblox : le jeu les reçoit comme celles d'un joueur, Studio n'a pas besoin d'être au premier plan, et ni le clavier ni la souris de l'utilisateur ne sont utilisés. Un clic `gui` sur un bouton vérifie qu'il s'est vraiment activé et échoue sinon (`verify: false` pour tester un bouton qui ne doit pas réagir). Un bouton d'interface en vue à la première personne doit avoir `Modal` pour libérer la souris, comme pour un joueur.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "steps": {
                        "type": "array",
                        "description": "Étapes jouées dans l'ordre (30 au plus). Chaque étape a un seul de ces champs : `key`, `gui`, `click`, `move`, `drag`, `look`, `scroll`, `text` ou `wait`. Les touches tenues sont toujours relâchées, même en cas d'erreur.",
                        "items": {
                            "type": "object",
                            "properties": {
                                "key": { "description": "Une touche ou une liste de touches tenues ensemble : lettres, chiffres, Space, Enter, Tab, Escape, Shift, Ctrl, Left/Right/Up/Down, F1 à F12." },
                                "hold": { "type": "integer", "description": "Durée d'appui en ms pour `key`, défaut 80, maximum 5000." },
                                "gui": { "type": "string", "description": "Chemin d'un GuiObject à cliquer en son centre, ex. `game.Players.LocalPlayer.PlayerGui.Menu.Jouer`." },
                                "click": { "type": "array", "items": { "type": "integer" }, "description": "[x, y] en pixels du viewport." },
                                "button": { "type": "string", "enum": ["left", "right", "middle"], "description": "Bouton pour `click`, `gui` et `drag`, défaut left." },
                                "move": { "type": "array", "items": { "type": "integer" }, "description": "[x, y] : amène le pointeur là sans cliquer, pour un survol." },
                                "drag": { "type": "object", "description": "Glisser : `{ \"from\": [x, y], \"to\": [x, y] }`, bouton enfoncé de l'un à l'autre.", "properties": { "from": { "type": "array", "items": { "type": "integer" } }, "to": { "type": "array", "items": { "type": "integer" } } } },
                                "look": { "type": "array", "items": { "type": "integer" }, "description": "[dx, dy] en pixels : tourne la caméra en glissant le bouton droit depuis le centre de l'écran, comme un joueur. Positif = vers la droite et vers le bas. Fonctionne aussi quand le jeu verrouille la souris (vue à la première personne)." },
                                "scroll": { "type": "integer", "description": "Crans de molette, positif = vers l'avant (zoom avant). Au centre de l'écran, ou au point `at`." },
                                "at": { "type": "array", "items": { "type": "integer" }, "description": "[x, y] pour `scroll`." },
                                "duration": { "type": "integer", "description": "Durée en ms de `drag` et `look`, défaut 400, maximum 5000." },
                                "text": { "type": "string", "description": "Texte tapé dans la zone de saisie qui a le focus (clique-la d'abord avec `gui`)." },
                                "verify": { "type": "boolean", "description": "Pour `gui` : false accepte qu'un bouton ne s'active pas." },
                                "wait": { "type": "integer", "description": "Pause en ms, maximum 5000." }
                            }
                        }
                    }
                },
                "required": ["steps"]
            }
        },
        {
            "name": "screenshot",
            "description": "Capture la vue 3D de Roblox Studio (le jeu pendant un test) et renvoie l'image. Fonctionne même si Studio est derrière d'autres fenêtres ; s'il est réduit, il est restauré en arrière-plan. Avec `frame`, la caméra vise une cible sous l'angle demandé le temps de la photo, puis revient exactement où elle était (position, champ de vision, type) : trois angles de contrôle = trois appels. Si un script du jeu reprend la caméra avant la photo, la réponse le dit.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "frame": frame.clone(),
                    "focus": { "type": "string", "description": "Raccourci de `frame: { target }` : chemin d'un Model ou d'une Part à cadrer de trois quarts." },
                    "area": { "type": "string", "enum": ["viewport", "window"], "description": "viewport = la vue 3D seule (défaut) ; window = toute la fenêtre Studio, avec l'explorateur et les panneaux." },
                    "max_width": { "type": "integer", "description": "Largeur maximale en pixels, défaut 1280." }
                }
            }
        },
        {
            "name": "capture_sequence",
            "description": "Photographie la vue 3D en rafale (`fps` images par seconde pendant `duration` secondes) et renvoie une planche-contact : les images de gauche à droite puis de haut en bas, chacune marquée de son numéro et de l'instant exact où elle a été prise. C'est le seul moyen de voir ce qui se passe dans le temps : une animation, un tween de caméra, un effet qui se déclenche, une transition d'interface, ou si quelque chose a bougé. `trigger_luau` lance l'action juste avant la première image.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "fps": { "type": "number", "description": "Images par seconde visées, 1 à 6, défaut 3. La cadence réellement obtenue est indiquée." },
                    "duration": { "type": "number", "description": "Secondes de rafale, 0,5 à 10, défaut 3. Vingt vignettes par planche ; au-delà, plusieurs planches." },
                    "trigger_luau": { "type": "string", "description": "Luau exécuté une fois juste avant la première image : jouer l'animation, déclencher l'effet. Sa sortie revient dans le texte." },
                    "trigger_context": context.clone(),
                    "frame": frame.clone()
                }
            }
        },
        {
            "name": "playtest_check",
            "description": "Vérifie une fonctionnalité dans un test neuf, en un seul appel, et rend un verdict. Dans l'ordre : attend que ton code soit compilé et arrivé dans Studio, lance le test, exécute `setup_luau`, fait marcher le personnage (`walk_to`), laisse tourner le jeu `duration` secondes, exécute `assert_luau`, lance les `checks`, lit les consoles serveur et client, prend une capture, puis arrête le test (même en cas d'échec). PASS : tout s'est exécuté, aucune ligne d'erreur dans les consoles, l'assertion tient. FAIL : la cause est nommée (étape en erreur, assertion fausse ou qui affiche FAIL, erreur dans la console). INCONCLUSIVE : le code testé n'a pas pu être garanti à jour (erreurs de compilation) ou le test n'a pas démarré. Un test déjà en cours doit être arrêté avant. `save_as` enregistre le scénario dans `.rovibe/tests/<nom>.json` ; `scenario` le rejoue.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "scenario": { "type": "string", "description": "Nom d'un scénario enregistré dans `.rovibe/tests/` (sans extension). Les autres champs passés le complètent ou le remplacent." },
                    "save_as": { "type": "string", "description": "Enregistre ce scénario sous ce nom pour le rejouer plus tard." },
                    "mode": { "type": "string", "enum": ["play", "run"], "description": "play = avec un joueur (défaut), run = serveur seul." },
                    "setup_luau": { "type": "string", "description": "Luau exécuté dès que le test est prêt : téléporter le joueur, faire apparaître un ennemi, donner un objet. Une erreur = FAIL." },
                    "setup_context": { "type": "string", "enum": ["server", "client"], "description": "Défaut server." },
                    "walk_to": { "description": "Après setup_luau, fait marcher le personnage : `[x, y, z]` ou chemin d'une instance. Ne pas arriver = FAIL." },
                    "duration": { "type": "number", "description": "Secondes pendant lesquelles le jeu tourne avant l'assertion, défaut 5, maximum 60." },
                    "assert_luau": { "type": "string", "description": "Luau exécuté juste avant l'arrêt : `print` ce qui compte. FAIL s'il lève une erreur, renvoie false ou affiche une ligne contenant FAIL en majuscules." },
                    "assert_context": { "type": "string", "enum": ["server", "client"], "description": "Défaut server." },
                    "checks": { "type": "array", "items": { "type": "object" }, "description": "Diagnostics lancés pendant le test, après l'assertion : `{ \"tool\": \"ui_check\" }` (client), `{ \"tool\": \"rig_check\", \"path\": \"…\" }`, `{ \"tool\": \"asset_check\", \"paths\": [\"…\"] }` (serveur). `context` les déplace. Leurs constats sont ajoutés au rapport sans changer le verdict." },
                    "screenshot": { "type": "boolean", "description": "Capture finale, défaut true en mode play." },
                    "wait_for_build": { "type": "boolean", "description": "Attendre que le code soit compilé et synchronisé avant de lancer, défaut true." }
                }
            }
        },
        {
            "name": "studio_wait",
            "description": "Attends ton tour pour piloter Studio. Un seul agent à la fois lance des tests, prend des captures ou joue au jeu dans une place ; quand une de ces actions t'est refusée parce qu'un autre agent pilote, appelle cet outil au lieu de réessayer en boucle. Il rend la main dès que c'est ton tour (les agents sont servis dans l'ordre d'arrivée), avec Studio déjà réservé pour toi. Si l'attente dépasse `timeout`, il te donne ta place dans la file : rappelle-le pour continuer d'attendre.",
            "inputSchema": {
                "type": "object",
                "properties": { "timeout": { "type": "number", "description": "Secondes d'attente maximale de cet appel, défaut 90." } }
            }
        },
        {
            "name": "studio_release",
            "description": "Rends la main sur Studio quand tu as fini de tester, pour qu'un autre agent puisse piloter. Facultatif : la main se libère seule environ 30 secondes après ta dernière action dans Studio (3 minutes pendant ton test).",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "selection",
            "description": "Ce que l'utilisateur a sélectionné dans Studio en ce moment. À appeler dès qu'il dit « ça », « cette pièce », « celui-là » sans donner de chemin : la sélection, c'est ce dont il parle. Avec `select`, sélectionne toi-même des instances pour lui montrer ton travail dans l'explorateur et la vue 3D.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "select": { "type": "array", "items": { "type": "string" }, "description": "Chemins à sélectionner, remplace la sélection actuelle. Liste vide = tout désélectionner." }
                }
            }
        },
        {
            "name": "rig_check",
            "description": "Vérifie qu'un modèle tient ensemble : chaque pièce reliée à la PrimaryPart par des soudures ou des Motor6D, joints cassés, pièces ancrées qui empêchent un Motor6D de bouger, Motor6D sans pivot, pièce pilotée deux fois, Humanoid et Animator. À lancer après avoir assemblé un personnage, un véhicule, une porte ou une machine.",
            "inputSchema": {
                "type": "object",
                "properties": { "path": { "type": "string" }, "context": context.clone() },
                "required": ["path"]
            }
        },
        {
            "name": "asset_check",
            "description": "Dimensions et composition de modèles ou de pièces (pièces, meshes, joints, scripts, ancrage), PrimaryPart droite ou non, et ce qu'il y a dessous : posé, qui flotte ou qui s'enfonce, et de combien. À lancer après avoir placé ou construit quelque chose.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "paths": { "type": "array", "items": { "type": "string" } },
                    "context": context.clone()
                },
                "required": ["paths"]
            }
        },
        {
            "name": "ui_check",
            "description": "Mesure une interface telle qu'elle est affichée : éléments qui sortent de l'écran, textes coupés, boutons trop petits pour un doigt, boutons qui se recouvrent, cadres de largeur fixe. En édition elle lit StarterGui ; pendant un test Play, le PlayerGui du joueur (contexte client par défaut).",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Limite la mesure à une ScreenGui ou un cadre." },
                    "context": context.clone()
                }
            }
        },
        {
            "name": "asset_search",
            "description": "Cherche des assets dans la banque locale (modèles enregistrés depuis Studio) et dans le Creator Store Roblox (gratuits uniquement). Renvoie des identifiants à passer à asset_insert : `bank:<id>` ou un numéro du Store.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "query": { "type": "string", "description": "Mots-clés. Vide = toute la banque locale." },
                    "collection": { "type": "string", "description": "Limite la banque locale à une collection." },
                    "source": { "type": "string", "enum": ["all", "bank", "store"], "description": "Défaut all." },
                    "type": { "type": "string", "enum": ["model", "audio", "decal", "mesh"], "description": "Type cherché dans le Store, défaut model." },
                    "limit": { "type": "integer", "description": "Défaut 10, maximum 30." }
                }
            }
        },
        {
            "name": "asset_insert",
            "description": "Insère un asset dans la place. Les scripts d'un asset du Store sont désactivés à l'insertion, sauf `keep_scripts: true` : un modèle gratuit peut contenir une porte dérobée, lis ses scripts avant de les activer.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "asset": { "type": "string", "description": "`bank:<id>` ou l'identifiant numérique d'un asset du Store." },
                    "type": { "type": "string", "enum": ["model", "audio", "decal", "mesh"], "description": "Type de l'asset du Store, défaut model." },
                    "parent": { "type": "string", "description": "Défaut game.Workspace." },
                    "name": { "type": "string" },
                    "position": { "type": "array", "items": { "type": "number" }, "description": "[x, y, z] du pivot." },
                    "keep_scripts": { "type": "boolean" }
                },
                "required": ["asset"]
            }
        },
        {
            "name": "asset_save",
            "description": "Enregistre une instance de la place (avec ses descendants) dans la banque locale, pour la réutiliser dans d'autres projets.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": { "type": "string", "description": "Ex. `game.Workspace.Arbre`." },
                    "name": { "type": "string", "description": "Défaut : le nom de l'instance." },
                    "tags": { "type": "array", "items": { "type": "string" } },
                    "collection": { "type": "string", "description": "Collection où ranger l'asset, ex. `Nature`. Créée si elle n'existe pas." }
                },
                "required": ["path"]
            }
        },
        {
            "name": "asset_preview",
            "description": "Renvoie l'image d'aperçu d'un asset, pour juger de son apparence avant de l'insérer. Pour un asset de la banque sans aperçu, l'image est prise dans Studio.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "asset": { "type": "string", "description": "`bank:<id>` ou l'identifiant numérique d'un asset du Store." }
                },
                "required": ["asset"]
            }
        },
        {
            "name": "sync_connect",
            "description": "Démarre le serveur de synchro du projet si besoin et y connecte Studio.",
            "inputSchema": { "type": "object", "properties": {} }
        }
    ])
}

fn as_text(value: Value) -> String {
    match value {
        Value::String(text) => text,
        other => serde_json::to_string_pretty(&other).unwrap_or_default(),
    }
}

async fn forward(
    state: &Shared,
    project: Option<&Project>,
    method: &str,
    args: &Value,
) -> Result<String, String> {
    let context = args["context"].as_str().unwrap_or("edit");
    let studio = studio::pick(state, project, context)?;
    studio
        .call(method, args.clone(), Duration::from_secs(30))
        .await
        .map(as_text)
}

/// Runs Luau in one DataModel and hands back the plugin's raw answer:
/// `ok`, `output`, `returns`, `error`.
async fn luau_call(state: &Shared, project: Option<&Project>, context: &str, code: &str, timeout: f64) -> Result<Value, String> {
    let timeout = timeout.clamp(1.0, 600.0);
    let studio = studio::pick(state, project, context)?;
    // The plugin enforces the timeout itself; the margin only covers a Studio
    // that stops answering altogether.
    studio
        .call(
            "run_luau",
            json!({ "code": code, "timeout": timeout }),
            Duration::from_secs_f64(timeout + 5.0),
        )
        .await
}

async fn run_luau(state: &Shared, project: Option<&Project>, args: &Value) -> Result<String, String> {
    let context = args["context"].as_str().unwrap_or("edit");
    let timeout = args["timeout"].as_f64().unwrap_or(30.0);
    let code = args["code"].as_str().ok_or("`code` est requis")?;
    let result = luau_call(state, project, context, code, timeout).await?;
    luau_text(&result)
}

/// What a run printed and returned, or why it failed.
fn luau_text(result: &Value) -> Result<String, String> {
    let mut text = result["output"].as_str().unwrap_or_default().to_owned();
    if result["ok"] == true {
        if let Some(returns) = result["returns"].as_array().filter(|list| !list.is_empty()) {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str("=> ");
            text.push_str(&serde_json::to_string(returns).unwrap_or_default());
        }
        if text.is_empty() {
            text.push_str("(aucune sortie)");
        }
        Ok(text)
    } else {
        if !text.is_empty() {
            text.push('\n');
        }
        text.push_str(result["error"].as_str().unwrap_or("erreur inconnue"));
        Err(text)
    }
}

/// The console as one agent reads it. Without `since` or `recent`, an agent
/// session gets what came after its own last read, so two agents never make
/// each other miss a line, and a long session isn't re-read on every call.
fn get_console(state: &Shared, project: Option<&Project>, session: Option<&str>, args: &Value) -> String {
    let recent = args["recent"].as_u64().map(|count| count.clamp(1, 1000) as usize);
    let own_cursor = session.and_then(|id| state.console_cursors.lock().unwrap().get(id).copied());
    let since = args["since"].as_u64().or(if recent.is_some() { None } else { own_cursor }).unwrap_or(0);
    let limit = recent.unwrap_or(args["limit"].as_u64().unwrap_or(100) as usize).max(1);
    let level = args["level"].as_str();
    let context = args["context"].as_str();
    let contains = args["contains"].as_str().map(str::to_lowercase);
    let place = project.and_then(|project| project.place_id);

    let logs = state.logs.lock().unwrap();
    let in_place = |entry: &&crate::state::LogEntry| place.is_none_or(|place| entry.place_id == place);
    let total = logs.entries.iter().filter(in_place).count();
    let matching: Vec<crate::state::LogEntry> = logs
        .entries
        .iter()
        .filter(in_place)
        .filter(|entry| recent.is_some() || entry.seq > since)
        .filter(|entry| level.is_none_or(|level| entry.level == level))
        .filter(|entry| context.is_none_or(|context| entry.context == context))
        .filter(|entry| {
            contains
                .as_ref()
                .is_none_or(|needle| entry.message.to_lowercase().contains(needle))
        })
        .cloned()
        .collect();
    let next = logs.next_seq;
    drop(logs);
    if let Some(id) = session {
        state.console_cursors.lock().unwrap().insert(id.to_owned(), next);
    }

    let mut text = String::new();
    if matching.is_empty() {
        let filtered = level.is_some() || context.is_some() || contains.is_some();
        text.push_str(&match (since > 0 && recent.is_none(), filtered) {
            (true, false) => format!("Aucune nouvelle ligne depuis ta dernière lecture ({total} lignes en tout dans la console)."),
            (true, true) => format!("Aucune nouvelle ligne ne correspond au filtre depuis ta dernière lecture ({total} lignes en tout)."),
            (false, false) => "La console est vide.".to_owned(),
            (false, true) => format!("Aucune ligne ne correspond au filtre ({total} lignes en tout)."),
        });
        text.push_str(&format!("\ncurseur: {next}"));
        return text;
    }

    let skipped = matching.len().saturating_sub(limit);
    let errors = matching.iter().filter(|entry| entry.level == "error").count();
    text.push_str(&format!(
        "{} ligne(s){}{}, {total} en tout dans la console\n",
        matching.len(),
        if recent.is_some() { " parmi les plus récentes" } else if since > 0 { " nouvelle(s)" } else { "" },
        match errors {
            0 => String::new(),
            n => format!(" dont {n} erreur(s)"),
        }
    ));
    if skipped > 0 {
        text.push_str(&format!("({skipped} lignes plus anciennes omises, `limit` pour en voir plus)\n"));
    }
    for entry in &matching[skipped..] {
        text.push_str(&format!(
            "[{}] {}: {}\n",
            entry.context, entry.level, entry.message
        ));
    }
    text.push_str(&format!("curseur: {next}"));
    text
}

fn studio_status(state: &Shared, project: Option<&Project>) -> String {
    let mut text = String::new();
    // Copied out and released at once: looking for a dialog asks Windows
    // about other programs' windows, and the sync state below has a lock of
    // its own. Holding this one across either is how two requests end up
    // waiting on each other for good.
    let studios: Vec<Arc<studio::Studio>> = state.studios.lock().unwrap().values().cloned().collect();
    if studios.is_empty() {
        text.push_str("Aucun Studio connecté.\n");
    }
    for studio in &studios {
        text.push_str(&format!(
            "Studio « {} » placeId={} contexte={}\n",
            studio.name, studio.place_id, studio.context
        ));
        if studio.context == "edit" {
            if let Some(title) = input::blocking_dialog(&studio.name) {
                text.push_str(&format!(
                    "  bloqué par {} : l'utilisateur doit y répondre dans Studio\n",
                    input::describe_dialog(&title)
                ));
            }
        }
    }

    if let Some(project) = project {
        let running = state.syncs.lock().unwrap().contains_key(&project.id);
        text.push_str(&format!(
            "Projet « {} » : synchro {} sur le port {}",
            project.name,
            if running { "active" } else { "arrêtée" },
            project.sync_port
        ));
        if let Some((_, title, playing)) = state.drives.holder(&project.id, session_gone(state)) {
            text.push_str(&format!(
                "\nStudio piloté par {title}{} : tests, captures et entrées de jeu lui sont réservés (`studio_wait` pour attendre ton tour)",
                if playing { ", test en cours" } else { "" }
            ));
        }
        if crate::typescript::is_project(&project.path) {
            let line = match sync::compiler(state, &project.id).map(|compiler| compiler.status()) {
                None => "\nTypeScript : compilateur arrêté (il tourne avec la synchro) ; `check_code` compile quand même".to_owned(),
                Some(status) if status.compiling => "\nTypeScript : compilation en cours".to_owned(),
                Some(status) => {
                    let errors = status.findings.iter().filter(|finding| finding.error).count();
                    if errors == 0 {
                        "\nTypeScript : compilé sans erreur, `out/` est à jour dans Studio".to_owned()
                    } else {
                        format!("\nTypeScript : {errors} erreur(s) de compilation, les fichiers concernés n'arrivent pas dans Studio (`check_code` pour le détail)")
                    }
                }
            };
            text.push_str(&line);
        }
    }
    text
}

pub async fn connect_sync(state: &Shared, project: &Project) -> Result<String, String> {
    let port = sync::serve(state, project).await?;
    state.notify();
    let studio = studio::pick(state, Some(project), "edit")?;

    // Syncing writes into the place: from here on the project belongs to
    // this one, and a second Studio opened later can't be mistaken for it.
    if !project.is_bound() {
        let mut projects = state.projects.lock().unwrap();
        if let Some(stored) = projects.iter_mut().find(|stored| stored.id == project.id) {
            if studio.place_id != 0 {
                stored.place_id = Some(studio.place_id);
            } else {
                stored.place_name = Some(studio.name.clone());
            }
        }
        drop(projects);
        let _ = state.save_projects();
    }
    studio
        .call(
            "sync_connect",
            json!({ "host": "localhost", "port": port }),
            Duration::from_secs(10),
        )
        .await?;
    Ok(if project.protected {
        format!("Studio se connecte à la synchro sur le port {port}. Le projet est protégé : Studio montre les changements à l'utilisateur, qui doit les accepter avant qu'ils s'appliquent.")
    } else {
        format!("Studio se connecte à la synchro sur le port {port}. Les fichiers du projet s'appliquent à la place sans confirmation.")
    })
}

async fn asset_search(args: &Value) -> Result<String, String> {
    let query = args["query"].as_str().unwrap_or_default().trim();
    let source = args["source"].as_str().unwrap_or("all");
    let kind = args["type"].as_str().unwrap_or("model");
    let limit = (args["limit"].as_u64().unwrap_or(10) as usize).clamp(1, 30);

    let mut text = String::new();
    if source != "store" {
        text.push_str("Banque locale :\n");
        let found = assets::search_bank(query, args["collection"].as_str(), limit);
        if found.is_empty() {
            text.push_str("(aucun résultat)\n");
        }
        for asset in found {
            text.push_str(&format!(
                "bank:{} | {} | {} | {} instances | {} Ko{}{}\n",
                asset.id,
                asset.name,
                if asset.class.is_empty() { "?" } else { &asset.class },
                asset.instances,
                asset.bytes / 1024,
                if asset.collection.is_empty() {
                    String::new()
                } else {
                    format!(" | collection : {}", asset.collection)
                },
                if asset.tags.is_empty() {
                    String::new()
                } else {
                    format!(" | tags : {}", asset.tags.join(", "))
                },
            ));
        }
    }

    if source != "bank" && !query.is_empty() {
        text.push_str(&format!("\nCreator Store, {kind}, gratuits :\n"));
        let found = assets::search_store(query, kind, limit).await?;
        if found.is_empty() {
            text.push_str("(aucun résultat)\n");
        }
        for item in &found {
            text.push_str(&store_line(item, kind));
        }
        text.push_str("\nasset_preview montre l'image d'un résultat avant de l'insérer.\n");
    }
    Ok(text)
}

fn store_line(item: &assets::StoreItem, kind: &str) -> String {
    let mut line = format!(
        "{} | {} | par {}{}",
        item.id,
        item.name,
        item.creator,
        if item.verified { " (vérifié)" } else { "" },
    );
    if let Some((percent, count)) = item.votes {
        line.push_str(&format!(" | {percent} % positifs sur {count} votes"));
    }
    if kind == "model" {
        if let Some(triangles) = item.triangles {
            line.push_str(&format!(" | {triangles} triangles"));
        }
        line.push_str(if item.has_scripts { " | CONTIENT DES SCRIPTS" } else { " | sans script" });
    }
    line.push('\n');
    line
}

fn asset_ref(args: &Value) -> Result<String, String> {
    // Accept a bare number as well: models tend to pass store ids unquoted.
    match &args["asset"] {
        Value::String(text) => Ok(text.trim().to_owned()),
        Value::Number(number) => Ok(number.to_string()),
        _ => Err("`asset` est requis".into()),
    }
}

async fn asset_preview(state: &Shared, project: Option<&Project>, args: &Value) -> Result<Vec<Value>, String> {
    let asset = asset_ref(args)?;
    let encode = |bytes: &[u8]| base64::engine::general_purpose::STANDARD.encode(bytes);

    if let Some(id) = asset.strip_prefix("bank:") {
        let path = assets::thumb_path(id).map_err(|error| error.to_string())?;
        if !path.exists() {
            let studio = studio::pick(state, project, "edit")?;
            make_preview(&studio, id).await?;
            state.notify();
        }
        let image = std::fs::read(path).map_err(|error| error.to_string())?;
        return Ok(vec![
            json!({ "type": "image", "data": encode(&image), "mimeType": "image/jpeg" }),
            json!({ "type": "text", "text": format!("Aperçu de bank:{id}") }),
        ]);
    }

    let id: u64 = asset
        .parse()
        .map_err(|_| "`asset` doit valoir `bank:<id>` ou un identifiant numérique du Store")?;
    let image = assets::store_preview(id).await?;
    Ok(vec![
        json!({ "type": "image", "data": encode(&image), "mimeType": "image/png" }),
        json!({ "type": "text", "text": format!("Aperçu de l'asset {id} du Creator Store") }),
    ])
}

/// Photographs a bank asset that has no preview: it is laid out in the place
/// away from everything, pictured, then taken out again. What Studio says
/// about it (class, size) fills the gaps of a file imported by hand.
pub async fn make_preview(studio: &studio::Studio, id: &str) -> Result<(), String> {
    let data = assets::read(id).map_err(|error| format!("Asset de banque illisible : {error}"))?;
    let staged = studio
        .call(
            "asset_stage",
            json!({ "data": base64::engine::general_purpose::STANDARD.encode(data) }),
            Duration::from_secs(60),
        )
        .await?;
    let _ = assets::edit(
        id,
        assets::Edit {
            class: staged["class"].as_str().map(str::to_owned),
            instances: staged["count"].as_u64(),
            ..assets::Edit::default()
        },
    );

    let pictured = if staged["visible"] == true {
        thumbnail(studio, staged["path"].as_str().unwrap_or_default(), id).await
    } else {
        Err("cet asset n'a rien à montrer dans la vue 3D".to_owned())
    };
    let _ = studio.call("asset_unstage", json!({}), Duration::from_secs(10)).await;
    pictured
}

pub async fn asset_insert(state: &Shared, project: Option<&Project>, args: &Value) -> Result<String, String> {
    let asset = asset_ref(args)?;

    let mut params = json!({
        "parent": args["parent"],
        "name": args["name"],
        "position": args["position"],
    });

    if let Some(id) = asset.strip_prefix("bank:") {
        let data = assets::read(id).map_err(|error| format!("Asset de banque illisible : {error}"))?;
        params["data"] = json!(base64::engine::general_purpose::STANDARD.encode(data));
    } else {
        let id: u64 = asset
            .parse()
            .map_err(|_| "`asset` doit valoir `bank:<id>` ou un identifiant numérique du Store")?;
        params["assetId"] = json!(id);
        params["kind"] = json!(args["type"].as_str().unwrap_or("model"));
        params["disableScripts"] = json!(args["keep_scripts"] != true);
    }

    let studio = studio::pick(state, project, "edit")?;
    studio
        .call("asset_insert", params, Duration::from_secs(90))
        .await
        .map(as_text)
}

async fn asset_save(state: &Shared, project: Option<&Project>, args: &Value) -> Result<String, String> {
    let studio = studio::pick(state, project, "edit")?;
    let exported = studio
        .call("asset_export", json!({ "path": args["path"] }), Duration::from_secs(60))
        .await?;

    let data = base64::engine::general_purpose::STANDARD
        .decode(exported["data"].as_str().unwrap_or_default())
        .map_err(|error| format!("Export illisible : {error}"))?;
    let name = args["name"]
        .as_str()
        .filter(|name| !name.trim().is_empty())
        .or(exported["name"].as_str())
        .unwrap_or("asset");
    let tags = args["tags"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|tag| tag.as_str().map(str::to_owned))
        .collect();
    let instances = exported["count"].as_u64().unwrap_or(0);

    let new = assets::NewAsset {
        name,
        tags,
        class: exported["class"].as_str().unwrap_or_default(),
        instances,
        collection: args["collection"].as_str().unwrap_or_default(),
    };
    let id = assets::save(new, &data).map_err(|error| error.to_string())?;

    let preview = match thumbnail(&studio, args["path"].as_str().unwrap_or_default(), &id).await {
        Ok(()) => ", avec aperçu",
        // The asset is saved either way; a preview is a convenience.
        Err(_) => ", sans aperçu",
    };
    state.notify();
    Ok(format!(
        "Enregistré dans la banque : bank:{id} ({instances} instances, {} Ko{preview})",
        data.len() / 1024
    ))
}

/// Photographs an instance for the bank, then puts the user's camera back
/// where it was.
async fn thumbnail(studio: &studio::Studio, path: &str, id: &str) -> Result<(), String> {
    let short = Duration::from_secs(10);
    let camera = studio.call("camera_get", json!({}), short).await?;
    let framed = studio.call("focus_camera", json!({ "path": path }), short).await;

    let shot = match framed {
        Ok(_) => {
            tokio::time::sleep(Duration::from_millis(400)).await;
            let size = (
                camera["width"].as_i64().unwrap_or(0) as i32,
                camera["height"].as_i64().unwrap_or(0) as i32,
            );
            let place = studio.name.clone();
            tokio::task::spawn_blocking(move || screenshot::capture(&place, Some(size), 360))
                .await
                .map_err(|error| error.to_string())?
        }
        Err(error) => Err(error),
    };
    let _ = studio.call("camera_set", camera, short).await;

    let shot = shot?;
    // A picture of the whole window would show panels, not the asset.
    if !shot.cropped {
        return Err("vue 3D introuvable".into());
    }
    let target = assets::thumb_path(id).map_err(|error| error.to_string())?;
    std::fs::write(target, shot.jpeg).map_err(|error| error.to_string())
}

/// Sends Studio its own "Publish to Roblox" shortcut. A plugin has no way to
/// publish, so this goes through the same door as the user.
pub async fn publish(state: &Shared, project: &Project) -> Result<String, String> {
    if studio::pick(state, Some(project), "server").is_ok() {
        return Err("Un test est en cours : arrête-le avant de publier".into());
    }
    let studio = studio::pick(state, Some(project), "edit")?;
    let info = studio.call("place_info", json!({}), Duration::from_secs(10)).await?;
    let size = (
        info["width"].as_i64().unwrap_or(0) as i32,
        info["height"].as_i64().unwrap_or(0) as i32,
    );
    let place = studio.name.clone();
    let place_id = info["placeId"].as_u64().unwrap_or(0);
    // Roblox's own record of the place, read before and after: the only
    // proof of a publication that doesn't depend on reading Studio's screen.
    let before = place_updated(place_id).await;

    let keys = state.settings.lock().unwrap().publish_keys();
    let dialog = tokio::task::spawn_blocking(move || {
        // From behind first; through the foreground, as a person would, if
        // Studio's keyboard state can't be reached.
        if let Err(error) = input::press_shortcut(&place, &keys) {
            crate::log::warn(format!("Publication en arrière-plan impossible ({error}) : Studio passe au premier plan"));
            let shortcut = [input::Step::Keys { keys, hold_ms: 80 }];
            input::run(&place, size, &shortcut)?;
        }
        std::thread::sleep(Duration::from_millis(1500));
        input::dialog_open(&place)
    })
    .await
    .map_err(|error| error.to_string())??;

    let confirmed = if place_id != 0 && !dialog && before.is_some() {
        let mut seen = None;
        for _ in 0..15 {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let now = place_updated(place_id).await;
            if now.is_some() && now != before {
                seen = now;
                break;
            }
        }
        Some(seen)
    } else {
        None
    };
    crate::log::info(format!(
        "Publication de « {} » (place {place_id}) demandée : dialogue={dialog}, confirmation={confirmed:?}",
        project.name
    ));

    Ok(match (place_id, dialog) {
        (id, false) if matches!(confirmed, Some(Some(_))) => format!(
            "Place {id} publiée : Roblox a enregistré la nouvelle version à {} (UTC).",
            confirmed.flatten().unwrap_or_default()
        ),
        (id, false) if matches!(confirmed, Some(None)) => format!(
            "Raccourci de publication envoyé pour la place {id}, mais Roblox n'a enregistré aucune nouvelle version en 45 s. Vérifie dans Studio : la publication a pu échouer ou attendre une réponse."
        ),
        (0, true) => "Cette place n'a jamais été publiée : Studio a ouvert sa fenêtre de publication, à terminer à la main (nom du jeu, créateur).".to_owned(),
        (0, false) => "Raccourci de publication envoyé, mais Studio n'a pas ouvert sa fenêtre de publication : publie cette place une première fois par Fichier > Publier sur Roblox.".to_owned(),
        (_, true) => "Raccourci de publication envoyé : Studio a ouvert une fenêtre qui attend une réponse, regarde-la.".to_owned(),
        (id, false) => format!("Publication de la place {id} lancée par le raccourci de Studio. Studio indique le résultat dans ses notifications ; l'app ne peut pas le lire."),
    })
}

/// When Roblox last recorded a new version of a place. A public figure, so
/// no key or login is involved.
async fn place_updated(place_id: u64) -> Option<String> {
    if place_id == 0 {
        return None;
    }
    let details: Value = reqwest::Client::new()
        .get(format!("https://economy.roblox.com/v2/assets/{place_id}/details"))
        .timeout(Duration::from_secs(8))
        .send()
        .await
        .ok()?
        .json()
        .await
        .ok()?;
    details["Updated"].as_str().map(str::to_owned)
}

/// The project as seen from an agent's own folder, if it has one.
fn in_folder(project: &Project, own: Option<&crate::pty::Worktree>) -> Project {
    let mut project = project.clone();
    if let Some(worktree) = own {
        project.path = worktree.dir.clone();
    }
    project
}

/// The `publish` tool: nothing happens until the user accepts in the app.
async fn request_publish(state: &Shared, project: &Project, session: Option<&str>) -> Result<String, String> {
    match ask_user(state, project, session, "publier la place sur Roblox").await {
        Some(true) => publish(state, project).await,
        Some(false) => Err("L'utilisateur a refusé la publication.".into()),
        None => Err("L'utilisateur n'a pas répondu à la demande de publication en 3 minutes : rien n'a été publié.".into()),
    }
}

/// `sync_connect` asked for by an agent. On a protected project the user
/// decides: connecting replaces the place's scripts with the project's files.
async fn request_sync(state: &Shared, project: &Project, session: Option<&str>) -> Result<String, String> {
    if project.protected {
        let request = "connecter la synchro : les fichiers du projet remplaceront les scripts de la place ouverte dans Studio";
        match ask_user(state, project, session, request).await {
            Some(true) => {}
            Some(false) => return Err("L'utilisateur a refusé de connecter la synchro de ce projet protégé.".into()),
            None => {
                return Err("Ce projet est protégé et l'utilisateur n'a pas répondu en 3 minutes : la synchro n'a pas été connectée.".into())
            }
        }
    }
    connect_sync(state, project).await
}

/// Shows a request in the app and waits for the user's answer; `None` when
/// they gave none in time.
async fn ask_user(state: &Shared, project: &Project, session: Option<&str>, request: &str) -> Option<bool> {
    let requester = session
        .and_then(|id| state.sessions.lock().unwrap().get(id).map(|session| session.info.title.clone()))
        .unwrap_or_else(|| "Un agent".to_owned());

    let (answer, decision) = tokio::sync::oneshot::channel();
    let id = state.next_id();
    state.approvals.lock().unwrap().insert(
        id,
        Approval {
            project_id: project.id.clone(),
            requester: requester.clone(),
            request: request.to_owned(),
            answer,
        },
    );
    state.announce(&requester, &format!("Demande à {request}"), true);
    let _ = state.attention.send(requester);
    state.notify();

    let allowed = tokio::time::timeout(Duration::from_secs(180), decision).await;
    state.approvals.lock().unwrap().remove(&id);
    state.notify();

    match allowed {
        Ok(Ok(allowed)) => Some(allowed),
        Ok(Err(_)) => Some(false),
        Err(_) => None,
    }
}

/// The DataModel whose 3D view is on screen: the client during a Play
/// test, whose layout (and so size) differs from the editor's, else the
/// editor.
fn viewer(state: &Shared, project: Option<&Project>) -> Result<Arc<studio::Studio>, String> {
    studio::pick(state, project, "client").or_else(|_| studio::pick(state, project, "edit"))
}

async fn viewport_of(viewer: &Result<Arc<studio::Studio>, String>) -> Option<(i32, i32)> {
    let size = viewer.as_ref().ok()?.call("viewport_info", json!({}), Duration::from_secs(10)).await.ok()?;
    Some((
        size["width"].as_i64().unwrap_or(0) as i32,
        size["height"].as_i64().unwrap_or(0) as i32,
    ))
}

/// A camera moved for a picture, to be put back once it is taken.
struct Framed {
    studio: Arc<studio::Studio>,
    before: Value,
    expected: Value,
}

impl Framed {
    /// Points `studio`'s camera as `frame` says, then gives Studio time to
    /// draw a frame from there.
    async fn new(studio: Arc<studio::Studio>, frame: Value) -> Result<Self, String> {
        let placed = studio.call("frame_camera", frame, Duration::from_secs(10)).await?;
        tokio::time::sleep(Duration::from_millis(350)).await;
        Ok(Self {
            studio,
            before: placed["before"].clone(),
            expected: placed["expected"].clone(),
        })
    }

    /// Whether the camera is still where it was put: a game's camera script
    /// takes it back within a frame, and the picture then shows the game's
    /// view, not the one asked for.
    async fn held(&self) -> bool {
        self.studio
            .call("camera_held", json!({ "expected": self.expected }), Duration::from_secs(10))
            .await
            .is_ok_and(|held| held == true)
    }

    async fn restore(self) {
        let _ = self.studio.call("camera_set", self.before, Duration::from_secs(10)).await;
    }
}

/// `frame`, or the older `focus` shortcut.
fn frame_of(args: &Value) -> Option<Value> {
    if args["frame"].is_object() {
        Some(args["frame"].clone())
    } else {
        args["focus"].as_str().map(|path| json!({ "target": path }))
    }
}

/// The 3D view as Roblox itself renders it: the view's DataModel takes the
/// picture, and the editor, the only one Roblox lets read a capture's
/// pixels, hands them over. No window, focus or screen involved.
async fn native_capture(state: &Shared, project: Option<&Project>) -> Result<image::RgbImage, String> {
    let edit = studio::pick(state, project, "edit")?;
    let viewer = viewer(state, project)?;
    let id = viewer.call("capture_start", json!({}), Duration::from_secs(10)).await?;
    let pixels = edit.call("capture_read", json!({ "id": id }), Duration::from_secs(20)).await?;
    let width = pixels["width"].as_u64().unwrap_or(0) as u32;
    let height = pixels["height"].as_u64().unwrap_or(0) as u32;
    let packed = base64::engine::general_purpose::STANDARD
        .decode(pixels["data"].as_str().unwrap_or_default())
        .map_err(|error| format!("capture illisible : {error}"))?;
    tokio::task::spawn_blocking(move || {
        let rgba = zstd::decode_all(&packed[..]).map_err(|error| format!("capture illisible : {error}"))?;
        screenshot::from_rgba(width, height, &rgba)
    })
    .await
    .map_err(|error| error.to_string())?
}

async fn take_screenshot(state: &Shared, project: Option<&Project>, args: &Value) -> Result<Vec<Value>, String> {
    // Without a connected Studio there is no place name to match, and the
    // capture falls back to the only Studio window if there is just one.
    let studio = studio::pick(state, project, "edit");
    let viewer = viewer(state, project);

    let framed = match frame_of(args) {
        Some(frame) => Some(Framed::new(viewer.clone()?, frame).await?),
        None => None,
    };

    let max_width = (args["max_width"].as_u64().unwrap_or(1280) as u32).clamp(320, 3840);
    let mut fallback = None;
    let native = if args["area"] != "window" {
        match native_capture(state, project).await {
            Ok(image) => Some(image),
            Err(error) => {
                fallback = Some(error);
                None
            }
        }
    } else {
        None
    };

    let rendered = native.is_some();
    let capture = match native {
        Some(image) => screenshot::encode(image, true, max_width),
        None => {
            let viewport = if args["area"] != "window" { viewport_of(&viewer).await } else { None };
            let place = studio.map(|studio| studio.name.clone()).unwrap_or_default();
            tokio::task::spawn_blocking(move || screenshot::capture(&place, viewport, max_width))
                .await
                .map_err(|error| error.to_string())
                .and_then(|capture| capture)
        }
    };

    let mut note = String::new();
    if let Some(error) = fallback {
        crate::log::warn(format!("Capture Roblox impossible ({error}) : capture de la fenêtre à la place"));
    }
    if let Some(framed) = framed {
        if !framed.held().await {
            note.push_str("\n⚠ Un script du jeu a repris la caméra avant la photo : l'image montre sa vue, pas le cadrage demandé.");
        }
        framed.restore().await;
        note.push_str("\nCaméra remise où elle était.");
    }
    let capture = capture?;

    let what = if capture.cropped { "Vue 3D de Studio" } else { "Fenêtre Studio entière" };
    let how = if rendered { ", rendue par Roblox" } else { "" };
    Ok(vec![
        json!({ "type": "image", "data": base64::engine::general_purpose::STANDARD.encode(&capture.jpeg), "mimeType": "image/jpeg" }),
        json!({ "type": "text", "text": format!("{what}{how}, {}x{} px{note}", capture.width, capture.height) }),
    ])
}

/// A burst of captures, returned as contact sheets.
async fn capture_sequence(state: &Shared, project: Option<&Project>, args: &Value) -> Result<Vec<Value>, String> {
    let fps = args["fps"].as_f64().unwrap_or(3.0).clamp(1.0, 6.0);
    let duration = args["duration"].as_f64().unwrap_or(3.0).clamp(0.5, 10.0);
    let place = studio::pick(state, project, "edit").map(|studio| studio.name.clone()).unwrap_or_default();
    let viewer = viewer(state, project);
    let viewport = viewport_of(&viewer).await;

    let framed = match frame_of(args) {
        Some(frame) => Some(Framed::new(viewer.clone()?, frame).await?),
        None => None,
    };

    // Run right before the first frame, so the burst starts at the action's
    // t=0 instead of whenever the agent guessed it would happen.
    let mut triggered = None;
    if let Some(code) = args["trigger_luau"].as_str() {
        let context = args["trigger_context"].as_str().unwrap_or("edit");
        match luau_call(state, project, context, code, 30.0).await.and_then(|result| luau_text(&result)) {
            Ok(text) => triggered = Some(text),
            Err(error) => {
                if let Some(framed) = framed {
                    framed.restore().await;
                }
                return Err(format!("trigger_luau a échoué, aucune image prise :\n{error}"));
            }
        }
    }

    let burst = tokio::task::spawn_blocking(move || {
        let start = std::time::Instant::now();
        let interval = 1.0 / fps;
        let mut frames = Vec::new();
        let mut next = 0.0;
        let mut skipped = 0;
        while start.elapsed().as_secs_f64() < duration {
            let at = start.elapsed().as_secs_f64();
            match screenshot::capture_raw(&place, viewport) {
                // Shrunk at once: sixty full-size frames would weigh hundreds
                // of megabytes for a sheet of thumbnails.
                Ok((image, _)) => frames.push((crate::sheet::shrink(&image), at)),
                Err(error) if frames.is_empty() => return Err(error),
                Err(_) => {}
            }
            next += interval;
            let now = start.elapsed().as_secs_f64();
            if now < next {
                std::thread::sleep(Duration::from_secs_f64(next - now));
            } else {
                // A capture slower than the cadence: skip the slots it ate
                // rather than bunching the next frames together.
                while next <= now {
                    next += interval;
                    skipped += 1;
                }
            }
        }
        Ok((frames, skipped))
    })
    .await
    .map_err(|error| error.to_string())
    .and_then(|burst| burst);

    let mut notes = Vec::new();
    if let Some(framed) = framed {
        if !framed.held().await {
            notes.push("⚠ Un script du jeu a repris la caméra pendant la rafale : les images montrent sa vue, pas le cadrage demandé.".to_owned());
        }
        framed.restore().await;
        notes.push("Caméra remise où elle était.".to_owned());
    }
    let (mut frames, skipped) = burst?;
    if frames.is_empty() {
        return Err("Aucune image prise".into());
    }
    let first_at = frames[0].1;
    for frame in &mut frames {
        frame.1 -= first_at;
    }
    let span = frames.last().map(|frame| frame.1).unwrap_or(0.0);
    let achieved = if span > 0.0 { (frames.len() - 1) as f64 / span } else { 0.0 };
    let sheets = crate::sheet::build(&frames, 0)?;

    let mut text = format!(
        "{} image(s) sur {:.2} s : cadence obtenue {:.1} i/s pour {:.0} visées{}. {} planche(s), de gauche à droite puis de haut en bas.\nInstants : {}",
        frames.len(),
        span,
        achieved,
        fps,
        if skipped > 0 { format!(", {skipped} créneau(x) sauté(s) car la capture était plus lente") } else { String::new() },
        sheets.len(),
        frames
            .iter()
            .enumerate()
            .map(|(index, (_, at))| format!("{}: {:.2}s", index + 1, at))
            .collect::<Vec<_>>()
            .join(", ")
    );
    for note in notes {
        text.push('\n');
        text.push_str(&note);
    }
    if let Some(output) = triggered {
        text.push_str("\ntrigger_luau : ");
        text.push_str(&output);
    }

    let mut content: Vec<Value> = sheets
        .iter()
        .map(|jpeg| json!({ "type": "image", "data": base64::engine::general_purpose::STANDARD.encode(jpeg), "mimeType": "image/jpeg" }))
        .collect();
    content.push(json!({ "type": "text", "text": text }));
    Ok(content)
}

/// Waits for the DataModels of a test that was just started, so the caller
/// can use them right away instead of polling studio_status.
async fn wait_for_test(state: &Shared, project: Option<&Project>, with_client: bool) -> String {
    if test_ready(state, project, with_client).await {
        if with_client {
            "Test démarré : serveur et client prêts.".to_owned()
        } else {
            "Test démarré : serveur prêt (mode run, sans joueur).".to_owned()
        }
    } else {
        "Test lancé, mais ses DataModels ne se sont pas connectés en 45 s : vérifie avec studio_status.".to_owned()
    }
}

/// Whether the DataModels of a test that was just started came up in time.
async fn test_ready(state: &Shared, project: Option<&Project>, with_client: bool) -> bool {
    let ready = |context: &str| studio::pick(state, project, context).is_ok();
    for _ in 0..180 {
        if ready("server") && (!with_client || ready("client")) {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    false
}

/// Ends the running test from its server DataModel, the only one allowed
/// to, and waits until it is gone.
async fn stop_test(state: &Shared, project: Option<&Project>) -> Result<(), String> {
    let server = studio::pick(state, project, "server")?;
    server.call("playtest", json!({ "action": "stop" }), Duration::from_secs(15)).await?;
    for _ in 0..60 {
        if studio::pick(state, project, "server").is_err() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    Err("le test tourne encore 15 s après la demande d'arrêt".into())
}

fn scenario_path(folder: &std::path::Path, name: &str) -> Result<std::path::PathBuf, String> {
    let valid = !name.is_empty() && name.len() <= 64 && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid {
        return Err("Un nom de scénario ne contient que des lettres, des chiffres, `-` et `_`".into());
    }
    Ok(folder.join(".rovibe").join("tests").join(format!("{name}.json")))
}

/// The scenario to run: a saved one if named, completed or overridden by
/// what the call passes, and saved under `save_as` if asked.
fn scenario(folder: &std::path::Path, args: &Value) -> Result<Value, String> {
    let mut spec = match args["scenario"].as_str() {
        Some(name) => {
            let path = scenario_path(folder, name)?;
            let bytes = std::fs::read(&path).map_err(|_| format!("Scénario introuvable : {}", path.display()))?;
            serde_json::from_slice(&bytes).map_err(|error| format!("Scénario {name} illisible : {error}"))?
        }
        None => json!({}),
    };
    if !spec.is_object() {
        return Err("Un scénario est un objet JSON".into());
    }
    for (key, value) in args.as_object().into_iter().flatten() {
        if key != "scenario" && key != "save_as" {
            spec[key] = value.clone();
        }
    }
    if let Some(name) = args["save_as"].as_str() {
        let path = scenario_path(folder, name)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        let json = serde_json::to_vec_pretty(&spec).map_err(|error| error.to_string())?;
        std::fs::write(&path, json).map_err(|error| error.to_string())?;
    }
    Ok(spec)
}

/// What a check found, for the verdict and for the agent.
#[derive(Default)]
struct Report {
    /// Each a reason for FAIL.
    failures: Vec<String>,
    /// Each a reason the result can't be trusted either way.
    doubts: Vec<String>,
    sections: Vec<String>,
}

impl Report {
    fn verdict(&self) -> String {
        if !self.failures.is_empty() {
            format!("FAIL — {}", self.failures.join(" ; "))
        } else if !self.doubts.is_empty() {
            format!("INCONCLUSIVE — {}", self.doubts.join(" ; "))
        } else {
            "PASS — tout s'est exécuté, aucune erreur dans les consoles, l'assertion tient.".to_owned()
        }
    }
}

/// Whether Studio holds the code on disk, as far as the app can tell.
async fn build_freshness(state: &Shared, project: &Project, folder: &std::path::Path, report: &mut Report) {
    let served = state.syncs.lock().unwrap().get(&project.id).map(|sync| sync.source.clone());
    let Some(source) = served else {
        report.doubts.push("la synchro est arrêtée : rien ne garantit que Studio a ton code (`sync_connect`)".into());
        return;
    };
    if source != folder {
        report.doubts.push(format!(
            "Studio reçoit le dossier {}, pas le tien : `sync_connect` pour y montrer ta branche",
            source.display()
        ));
        return;
    }
    if crate::typescript::is_project(folder) {
        match sync::compiler(state, &project.id) {
            None => report.doubts.push("le compilateur TypeScript ne tourne pas : le Luau dans Studio peut dater".into()),
            Some(watch) => match watch.settle(Duration::from_secs(60)).await {
                None => report.doubts.push("la compilation TypeScript n'était pas finie après 60 s".into()),
                Some(findings) => {
                    let errors = findings.iter().filter(|finding| finding.error).count();
                    if errors > 0 {
                        report.doubts.push(format!(
                            "{errors} erreur(s) de compilation : les fichiers concernés n'ont pas atteint Studio, qui teste leur dernière version compilée (`check_code`)"
                        ));
                    } else {
                        report.sections.push("Build : TypeScript compilé sans erreur.".into());
                    }
                }
            },
        }
    }
    // Long enough for the sync to carry the last save into the place.
    tokio::time::sleep(Duration::from_millis(600)).await;
}

/// Runs one step's Luau and records it; `strict` also fails a step that
/// returns false or prints FAIL, which is how an assertion says no.
async fn check_step(state: &Shared, project: &Project, report: &mut Report, step: &str, code: &str, context: &str, strict: bool) {
    match luau_call(state, Some(project), context, code, 30.0).await {
        Err(error) => {
            report.failures.push(format!("{step} n'a pas pu s'exécuter ({error})"));
            report.sections.push(format!("{step} ({context}) : {error}"));
        }
        Ok(result) => {
            let text = match luau_text(&result) {
                Ok(text) => text,
                Err(text) => {
                    report.failures.push(format!("{step} a levé une erreur"));
                    report.sections.push(format!("{step} ({context}) :\n{text}"));
                    return;
                }
            };
            if strict {
                let returned_false = result["returns"].as_array().and_then(|list| list.first()) == Some(&Value::Bool(false));
                let printed_fail = result["output"]
                    .as_str()
                    .unwrap_or_default()
                    .lines()
                    .any(|line| line.contains("FAIL"));
                if returned_false {
                    report.failures.push(format!("{step} a renvoyé false"));
                } else if printed_fail {
                    report.failures.push(format!("{step} a affiché FAIL"));
                }
            }
            report.sections.push(format!("{step} ({context}) :\n{text}"));
        }
    }
}

/// The console lines of one test DataModel since `after`.
fn test_console(state: &Shared, project: &Project, context: &str, after: u64) -> (usize, String) {
    let logs = state.logs.lock().unwrap();
    let lines: Vec<_> = logs
        .entries
        .iter()
        .filter(|entry| entry.seq > after && entry.context == context)
        .filter(|entry| project.place_id.is_none_or(|place| entry.place_id == place))
        .collect();
    let errors = lines.iter().filter(|entry| entry.level == "error").count();
    let shown = lines.len().saturating_sub(40);
    let mut text = format!("Console {context} : {} ligne(s), {errors} erreur(s)", lines.len());
    if shown > 0 {
        text.push_str(&format!(" ({shown} plus anciennes omises)"));
    }
    for entry in &lines[shown..] {
        text.push_str(&format!("\n  {}: {}", entry.level, entry.message));
    }
    (errors, text)
}

/// `playtest_check`: a fresh test, run start to finish, with a verdict.
/// Spawned, so that an agent cancelling its call can't leave a test running.
async fn playtest_check(
    state: &Shared,
    project: Option<&Project>,
    session: Option<&str>,
    own: Option<&crate::pty::Worktree>,
    args: &Value,
) -> Result<Vec<Value>, String> {
    let project = project.ok_or("Cette session n'est rattachée à aucun projet")?.clone();
    let folder = in_folder(&project, own).path;
    let spec = scenario(&folder, args)?;
    let state = state.clone();
    let session = session.map(str::to_owned);
    tokio::spawn(async move { run_check(&state, &project, session.as_deref(), &folder, &spec).await })
        .await
        .map_err(|error| error.to_string())?
}

async fn run_check(state: &Shared, project: &Project, session: Option<&str>, folder: &std::path::Path, spec: &Value) -> Result<Vec<Value>, String> {
    let play = spec["mode"] != "run";
    if studio::pick(state, Some(project), "server").is_ok() {
        return Err("Un test est déjà en cours dans Studio : arrête-le (`playtest` avec `action: \"stop\"`) avant playtest_check, qui lance le sien.".into());
    }
    let edit = studio::pick(state, Some(project), "edit")?;
    if let Some(title) = input::blocking_dialog(&edit.name) {
        return Err(format!(
            "Studio est bloqué par {} : l'utilisateur doit y répondre avant de lancer un test.",
            input::describe_dialog(&title)
        ));
    }

    let mut report = Report::default();
    if spec["wait_for_build"] != false {
        build_freshness(state, project, folder, &mut report).await;
    }

    let after = state.logs.lock().unwrap().next_seq;
    let started = std::time::Instant::now();
    edit.call("playtest", json!({ "action": "start", "mode": if play { "play" } else { "run" } }), Duration::from_secs(15))
        .await?;
    if let Some(session) = session {
        state.drives.set_playing(&project.id, session, true);
    }

    let mut images = Vec::new();
    if !test_ready(state, Some(project), play).await {
        report.doubts.push("le test n'a pas démarré en 45 s".into());
    } else {
        if let Some(code) = spec["setup_luau"].as_str() {
            let context = spec["setup_context"].as_str().unwrap_or("server");
            check_step(state, project, &mut report, "setup_luau", code, context, false).await;
        }

        if !spec["walk_to"].is_null() {
            if !play {
                report.failures.push("walk_to demande le mode play".into());
            } else {
                match studio::pick(state, Some(project), "client") {
                    Err(error) => report.failures.push(format!("walk_to : {error}")),
                    Ok(client) => match client
                        .call("play_move", json!({ "to": spec["walk_to"], "timeout": 20 }), Duration::from_secs(30))
                        .await
                    {
                        Err(error) => report.failures.push(format!("walk_to a échoué ({error})")),
                        Ok(moved) => {
                            if moved["arrived"] != true {
                                report.failures.push("le personnage n'est pas arrivé à destination".into());
                            }
                            report.sections.push(format!("Marche : {}", moved["text"].as_str().unwrap_or_default()));
                        }
                    },
                }
            }
        }

        let duration = spec["duration"].as_f64().unwrap_or(5.0).clamp(0.0, 60.0);
        let elapsed = started.elapsed().as_secs_f64();
        if duration > elapsed {
            tokio::time::sleep(Duration::from_secs_f64(duration - elapsed)).await;
        }

        if let Some(code) = spec["assert_luau"].as_str() {
            let context = spec["assert_context"].as_str().unwrap_or("server");
            check_step(state, project, &mut report, "assert_luau", code, context, true).await;
        }

        for check in spec["checks"].as_array().into_iter().flatten() {
            let tool = check["tool"].as_str().unwrap_or_default();
            if !matches!(tool, "ui_check" | "rig_check" | "asset_check") {
                report.failures.push(format!("diagnostic inconnu : {tool:?} (ui_check, rig_check ou asset_check)"));
                continue;
            }
            let fallback = if tool == "ui_check" { "client" } else { "server" };
            let context = check["context"].as_str().unwrap_or(fallback);
            let outcome = match studio::pick(state, Some(project), context) {
                Ok(studio) => studio.call(tool, check.clone(), Duration::from_secs(30)).await.map(as_text),
                Err(error) => Err(error),
            };
            match outcome {
                Ok(text) => report.sections.push(format!("{tool} ({context}) :\n{text}")),
                Err(error) => report.failures.push(format!("{tool} n'a pas pu s'exécuter ({error})")),
            }
        }

        // The game's own errors, read once everything above had its chance
        // to cause them, and once the plugin's last batch of lines (sent
        // every 100 ms) has arrived.
        tokio::time::sleep(Duration::from_millis(400)).await;
        let mut contexts = vec!["server"];
        if play {
            contexts.push("client");
        }
        for context in contexts {
            let (errors, text) = test_console(state, project, context, after);
            if errors > 0 {
                report.failures.push(format!("{errors} erreur(s) dans la console {context}"));
            }
            report.sections.push(text);
        }

        if play && spec["screenshot"] != false {
            match take_screenshot(state, Some(project), &json!({})).await {
                Ok(content) => images = content,
                Err(error) => report.sections.push(format!("Capture impossible : {error}")),
            }
        }
    }

    // A test that never came up has nothing to stop.
    let stopped = if studio::pick(state, Some(project), "server").is_ok() {
        stop_test(state, Some(project)).await
    } else {
        Ok(())
    };
    if let Some(session) = session {
        state.drives.set_playing(&project.id, session, false);
    }
    match stopped {
        Ok(()) => report.sections.insert(0, format!("Mode {}, test arrêté après {:.0} s.", if play { "play" } else { "run" }, started.elapsed().as_secs_f64())),
        Err(error) => {
            report.doubts.push(format!("l'arrêt du test a échoué ({error}) : arrête-le avec `playtest` avant de continuer"));
        }
    }

    let mut text = report.verdict();
    for section in &report.sections {
        text.push_str("\n\n");
        text.push_str(section);
    }
    let mut content = vec![json!({ "type": "text", "text": text })];
    // The picture goes after the report, whose first line is the verdict.
    content.extend(images.into_iter().filter(|item| item["type"] == "image"));
    Ok(content)
}

/// The agent behind a session, as other agents are told about it.
fn session_title(state: &Shared, session: &str) -> String {
    state
        .sessions
        .lock()
        .unwrap()
        .get(session)
        .map(|session| session.info.title.clone())
        .unwrap_or_else(|| "un agent".to_owned())
}

/// Says whether a session is closed, so that it never keeps Studio.
fn session_gone(state: &Shared) -> impl Fn(&str) -> bool + '_ {
    move |id| {
        state
            .sessions
            .lock()
            .unwrap()
            .get(id)
            .is_none_or(|session| session.exited.load(std::sync::atomic::Ordering::Relaxed))
    }
}

/// Tools that act on Studio's live session: one agent at a time per place.
/// Reading the place or running code in it while editing is shared.
fn drives_studio(name: &str, args: &Value) -> bool {
    match name {
        "playtest" | "playtest_check" | "play_move" | "play_input" | "screenshot" | "capture_sequence" => true,
        "run_luau" => matches!(args["context"].as_str(), Some("server" | "client")),
        _ => false,
    }
}

async fn studio_wait(state: &Shared, project: Option<&Project>, session: Option<&str>, args: &Value) -> Result<String, String> {
    let project = project.ok_or("Cette session n'est rattachée à aucun projet")?;
    let session = session.ok_or("L'attente demande une session lancée depuis RoVibe")?;
    let limit = Duration::from_secs_f64(args["timeout"].as_f64().unwrap_or(90.0).clamp(1.0, 600.0));
    let title = session_title(state, session);
    match state.drives.wait(&project.id, session, &title, session_gone(state), limit).await {
        crate::drive::Waited::Granted => Ok("C'est ton tour : Studio est réservé pour toi. Lance ton action maintenant ; la main se libère seule environ 30 s après ta dernière action dans Studio, ou par `studio_release`.".into()),
        crate::drive::Waited::Queued(position, busy) => Ok(format!(
            "Pas encore ton tour : tu es {} dans la file. {} pilote toujours. Rappelle `studio_wait` pour continuer d'attendre, ta place est gardée.",
            if position == 1 { "1er".to_owned() } else { format!("{position}e") },
            busy.title
        )),
    }
}

async fn play_input(state: &Shared, project: Option<&Project>, args: &Value) -> Result<String, String> {
    let client = studio::pick(state, project, "client")
        .map_err(|_| "play_input demande un test Play en cours (outil playtest, mode play)".to_owned())?;

    let listed = args["steps"].as_array().ok_or("`steps` est requis")?;
    if listed.is_empty() || listed.len() > 30 {
        return Err("`steps` doit contenir entre 1 et 30 étapes".into());
    }

    // Roblox's VirtualInput when this Studio has it: engine input that the
    // game can't tell from a player's, keyboard layout and window aside.
    let native = client
        .call("vinput_available", json!({}), Duration::from_secs(10))
        .await
        .is_ok_and(|available| available == true);
    if native {
        let budget: u64 = listed
            .iter()
            .map(|step| {
                ["hold", "duration", "wait"]
                    .iter()
                    .filter_map(|key| step[*key].as_u64())
                    .map(|ms| ms.min(5000))
                    .sum::<u64>()
                    + 500
            })
            .sum();
        let observed = client
            .call("vinput", json!({ "steps": listed }), Duration::from_millis(budget + 15_000))
            .await?;
        let observed = observed.as_str().unwrap_or_default();
        return Ok(format!(
            "{} étape(s) jouée(s) dans le jeu par VirtualInput (entrées du moteur Roblox, en arrière-plan).{}{}",
            listed.len(),
            if observed.is_empty() { "" } else { "\n" },
            observed
        ));
    }
    if listed.iter().any(|step| !step["text"].is_null()) {
        return Err("`text` demande VirtualInput, que ce Studio n'a pas : mets Roblox Studio à jour".into());
    }

    let viewport = client
        .call("viewport_info", json!({}), Duration::from_secs(10))
        .await?;
    let size = (
        viewport["width"].as_i64().unwrap_or(0) as i32,
        viewport["height"].as_i64().unwrap_or(0) as i32,
    );
    let center = (size.0 / 2, size.1 / 2);
    let point = |value: &Value| -> Option<(i32, i32)> {
        let pair = value.as_array()?;
        Some((pair.first()?.as_i64()? as i32, pair.get(1)?.as_i64()? as i32))
    };

    let mut steps = Vec::new();
    for step in listed {
        let button = input::Button::named(step["button"].as_str())?;
        let duration = step["duration"].as_u64().unwrap_or(400).clamp(50, 5000);

        if !step["key"].is_null() {
            let keys: Vec<String> = match &step["key"] {
                Value::String(key) => vec![key.clone()],
                Value::Array(keys) => keys.iter().filter_map(|key| key.as_str().map(str::to_owned)).collect(),
                _ => return Err("`key` attend une touche ou une liste de touches".into()),
            };
            steps.push(input::Step::Keys {
                keys,
                hold_ms: step["hold"].as_u64().unwrap_or(80).min(5000),
            });
        } else if let Some(path) = step["gui"].as_str() {
            // Asked now rather than up front: an earlier step may be what
            // makes this element appear.
            let center = client
                .call("gui_center", json!({ "path": path }), Duration::from_secs(10))
                .await?;
            steps.push(input::Step::Click {
                x: center["x"].as_i64().unwrap_or(0) as i32,
                y: center["y"].as_i64().unwrap_or(0) as i32,
                button,
            });
        } else if !step["click"].is_null() {
            let (x, y) = point(&step["click"]).ok_or("`click` attend [x, y]")?;
            steps.push(input::Step::Click { x, y, button });
        } else if !step["move"].is_null() {
            let (x, y) = point(&step["move"]).ok_or("`move` attend [x, y]")?;
            steps.push(input::Step::Move { x, y });
        } else if !step["drag"].is_null() {
            let from = point(&step["drag"]["from"]).ok_or("`drag` attend { from: [x, y], to: [x, y] }")?;
            let to = point(&step["drag"]["to"]).ok_or("`drag` attend { from: [x, y], to: [x, y] }")?;
            steps.push(input::Step::Drag { from, to, button, ms: duration });
        } else if !step["look"].is_null() {
            // What a player does to turn the default camera: hold the right
            // button and move. The travel is kept inside the view.
            let (dx, dy) = point(&step["look"]).ok_or("`look` attend [dx, dy]")?;
            let to = (
                (center.0 + dx).clamp(1, (size.0 - 2).max(1)),
                (center.1 + dy).clamp(1, (size.1 - 2).max(1)),
            );
            steps.push(input::Step::Drag { from: center, to, button: input::Button::Right, ms: duration });
        } else if let Some(notches) = step["scroll"].as_i64() {
            let (x, y) = point(&step["at"]).unwrap_or(center);
            steps.push(input::Step::Scroll { x, y, notches: notches.clamp(-40, 40) as i32 });
        } else if let Some(ms) = step["wait"].as_u64() {
            steps.push(input::Step::Wait(ms.min(5000)));
        } else {
            return Err("Chaque étape doit avoir `key`, `gui`, `click`, `move`, `drag`, `look`, `scroll` ou `wait`".into());
        }
    }

    // The window title carries the place name of the edit DataModel.
    let place = studio::pick(state, project, "edit")
        .map(|studio| studio.name.clone())
        .unwrap_or_default();
    let count = steps.len();

    tokio::task::spawn_blocking(move || input::run_in_game(&place, size, &steps))
        .await
        .map_err(|error| error.to_string())??;
    Ok(format!("{count} étape(s) jouée(s) dans le jeu."))
}

async fn call_tool(
    state: &Shared,
    project: Option<&Project>,
    name: &str,
    args: &Value,
) -> Result<String, String> {
    match name {
        "studio_status" => Ok(studio_status(state, project)),
        "run_luau" => run_luau(state, project, args).await,
        "get_tree" | "search" | "get_instance" | "rig_check" | "asset_check" => forward(state, project, name, args).await,
        "selection" => {
            let studio = studio::pick(state, project, "edit")?;
            studio.call("selection", args.clone(), Duration::from_secs(10)).await.map(as_text)
        }
        "ui_check" => {
            // During a Play test the interface players see is the client's.
            let mut args = args.clone();
            if args["context"].is_null() && studio::pick(state, project, "client").is_ok() {
                args["context"] = json!("client");
            }
            forward(state, project, name, &args).await
        }
        "playtest" => {
            let stopping = args["action"] == "stop";
            // Only the server DataModel is allowed to end a running test.
            let context = if stopping { "server" } else { "edit" };
            let studio = studio::pick(state, project, context)?;
            if !stopping {
                // Studio queues the test behind an open dialog and the tool
                // would then wait for DataModels that never come.
                if let Some(title) = input::blocking_dialog(&studio.name) {
                    return Err(format!(
                        "Studio est bloqué par {} : l'utilisateur doit y répondre avant de lancer un test.",
                        input::describe_dialog(&title)
                    ));
                }
            }
            let reply = studio
                .call("playtest", args.clone(), Duration::from_secs(15))
                .await
                .map(as_text)?;
            if stopping {
                Ok(reply)
            } else {
                Ok(wait_for_test(state, project, args["mode"] != "run").await)
            }
        }
        "play_move" => {
            let client = studio::pick(state, project, "client")
                .map_err(|_| "play_move demande un test Play en cours (outil playtest, mode play)".to_owned())?;
            let timeout = args["timeout"].as_f64().unwrap_or(20.0).clamp(1.0, 120.0);
            let moved = client
                .call(
                    "play_move",
                    json!({ "to": args["to"], "timeout": timeout }),
                    Duration::from_secs_f64(timeout + 10.0),
                )
                .await?;
            Ok(moved["text"].as_str().map(str::to_owned).unwrap_or_else(|| as_text(moved)))
        }
        "play_input" => play_input(state, project, args).await,
        "asset_search" => asset_search(args).await,
        "asset_insert" => asset_insert(state, project, args).await,
        "asset_save" => asset_save(state, project, args).await,
        "check_code" => {
            let project = project.ok_or("Cette session n'est rattachée à aucun projet")?;
            let paths: Vec<String> = args["paths"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|path| path.as_str().map(str::to_owned))
                .collect();
            lint::check(state, project, &paths, args["warnings"] == true).await
        }
        other => Err(format!("Outil inconnu : {other}")),
    }
}

fn coordinate(
    state: &Shared,
    project: Option<&Project>,
    session: Option<&str>,
    name: &str,
    args: &Value,
) -> Result<String, String> {
    let project = project.ok_or("Cette session n'est rattachée à aucun projet")?;
    if name == "agents_status" {
        return Ok(agents::describe(state, &project.id, session));
    }

    let session = session.ok_or("Les réservations demandent une session lancée depuis RoVibe")?;
    match name {
        "claim_files" => agents::claim(state, project, session, args),
        _ => Ok(agents::release(state, project, session, args)),
    }
}

pub async fn handler(
    State(state): State<Shared>,
    Path((token, project_id)): Path<(String, String)>,
    Json(request): Json<Value>,
) -> Response {
    serve(state, token, project_id, None, request).await
}

/// Same server, reached through the address of one agent session, which is
/// what lets coordination tools know who is calling.
pub async fn session_handler(
    State(state): State<Shared>,
    Path((token, project_id, session_id)): Path<(String, String, String)>,
    Json(request): Json<Value>,
) -> Response {
    serve(state, token, project_id, Some(session_id), request).await
}

async fn serve(
    state: Shared,
    token: String,
    project_id: String,
    session: Option<String>,
    request: Value,
) -> Response {
    if token != state.token {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    let id = request["id"].clone();
    let method = request["method"].as_str().unwrap_or_default();

    // Notifications carry no id and expect no body.
    if id.is_null() {
        return StatusCode::ACCEPTED.into_response();
    }

    let result = match method {
        "initialize" => json!({
            "protocolVersion": request["params"]["protocolVersion"].as_str().unwrap_or(DEFAULT_PROTOCOL),
            "capabilities": { "tools": {} },
            "serverInfo": { "name": "rovibe-mcp", "version": env!("CARGO_PKG_VERSION") },
            "instructions": INSTRUCTIONS,
        }),
        "ping" => json!({}),
        "tools/list" => json!({ "tools": tools() }),
        "tools/call" => {
            let project = state.project(&project_id);
            // An agent with a folder of its own is checked and synced from
            // there, not from the project's.
            let own = session
                .as_deref()
                .and_then(|id| state.sessions.lock().unwrap().get(id).and_then(|session| session.worktree.clone()));
            let params = &request["params"];
            let name = params["name"].as_str().unwrap_or_default();
            let arguments = &params["arguments"];
            let as_content = |text: String| vec![json!({ "type": "text", "text": text })];
            // One agent drives a place at a time; the others hear who does.
            let driving = match (project.as_ref(), session.as_deref()) {
                (Some(project), Some(session)) if drives_studio(name, arguments) => Some((project.id.clone(), session.to_owned())),
                _ => None,
            };
            let refused = driving.as_ref().and_then(|(place, session)| {
                let title = session_title(&state, session);
                state.drives.acquire(place, session, &title, session_gone(&state)).err()
            });
            let outcome = if let Some(busy) = refused {
                Err(busy.message())
            } else {
                match name {
                "screenshot" => take_screenshot(&state, project.as_ref(), arguments).await,
                "capture_sequence" => capture_sequence(&state, project.as_ref(), arguments).await,
                "playtest_check" => playtest_check(&state, project.as_ref(), session.as_deref(), own.as_ref(), arguments).await,
                "get_console" => Ok(as_content(get_console(&state, project.as_ref(), session.as_deref(), arguments))),
                "studio_wait" => studio_wait(&state, project.as_ref(), session.as_deref(), arguments).await.map(as_content),
                "studio_release" => match (project.as_ref(), session.as_deref()) {
                    (Some(project), Some(session)) => Ok(as_content(if state.drives.release(&project.id, session) {
                        "Studio est libre pour les autres agents.".to_owned()
                    } else {
                        "Tu ne pilotais pas Studio : rien à rendre.".to_owned()
                    })),
                    _ => Err("Cette session n'est rattachée à aucun projet".to_owned()),
                },
                "asset_preview" => asset_preview(&state, project.as_ref(), arguments).await,
                "publish" => match project.as_ref() {
                    Some(project) => request_publish(&state, project, session.as_deref())
                        .await
                        .map(as_content),
                    None => Err("Cette session n'est rattachée à aucun projet".to_owned()),
                },
                "sync_connect" => match project.as_ref() {
                    Some(project) => request_sync(&state, &in_folder(project, own.as_ref()), session.as_deref())
                        .await
                        .map(as_content),
                    None => Err("Cette session n'est rattachée à aucun projet".to_owned()),
                },
                "agents_status" | "claim_files" | "release_files" => {
                    coordinate(&state, project.as_ref(), session.as_deref(), name, arguments)
                        .map(as_content)
                }
                "check_code" => {
                    let project = project.as_ref().map(|project| in_folder(project, own.as_ref()));
                    call_tool(&state, project.as_ref(), name, arguments).await.map(as_content)
                }
                _ => call_tool(&state, project.as_ref(), name, arguments)
                    .await
                    .map(as_content),
                }
            };
            if let (Some((place, session)), Ok(_)) = (&driving, &outcome) {
                // A running test keeps its place longer than a screenshot.
                if name == "playtest" {
                    state.drives.set_playing(place, session, arguments["action"] != "stop");
                }
            }
            match outcome {
                Ok(content) => json!({ "content": content, "isError": false }),
                Err(text) => {
                    crate::log::warn(format!("Outil {name} : {text}"));
                    json!({ "content": [{ "type": "text", "text": text }], "isError": true })
                }
            }
        }
        _ => {
            return Json(json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": { "code": -32601, "message": format!("Method not found: {method}") }
            }))
            .into_response();
        }
    };

    Json(json!({ "jsonrpc": "2.0", "id": id, "result": result })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::AppState;

    fn empty_state() -> Shared {
        Arc::new(AppState::new(0, "t".into(), std::env::temp_dir(), "0".into()))
    }

    #[test]
    fn each_agent_reads_the_console_from_where_it_left_off() {
        let state = empty_state();
        let log = |message: &str, level: &str| {
            state.logs.lock().unwrap().push(level.into(), "server".into(), 0, message.into());
        };
        log("un", "output");
        log("deux", "error");

        let first = get_console(&state, None, Some("a"), &json!({}));
        assert!(first.contains("un") && first.contains("deux") && first.contains("dont 1 erreur"), "{first}");
        let again = get_console(&state, None, Some("a"), &json!({}));
        assert!(again.starts_with("Aucune nouvelle ligne"), "{again}");

        // Another agent has read nothing yet: it gets everything.
        let other = get_console(&state, None, Some("b"), &json!({}));
        assert!(other.contains("deux"), "{other}");

        log("trois", "output");
        let next = get_console(&state, None, Some("a"), &json!({}));
        assert!(next.contains("trois") && !next.contains("deux"), "{next}");
        let recent = get_console(&state, None, Some("a"), &json!({ "recent": 2 }));
        assert!(recent.contains("deux") && recent.contains("trois") && !recent.contains("un\n"), "{recent}");
        let filtered = get_console(&state, None, Some("a"), &json!({ "context": "client" }));
        assert!(filtered.contains("ne correspond au filtre"), "{filtered}");
    }

    #[test]
    fn a_scenario_is_saved_then_replayed_with_overrides() {
        let folder = tempfile::tempdir().unwrap();
        let saved = scenario(folder.path(), &json!({ "save_as": "porte", "setup_luau": "print(1)", "duration": 3 })).unwrap();
        assert_eq!(saved["duration"], 3);
        assert!(folder.path().join(".rovibe/tests/porte.json").exists());

        let replayed = scenario(folder.path(), &json!({ "scenario": "porte", "duration": 8 })).unwrap();
        assert_eq!(replayed["setup_luau"], "print(1)");
        assert_eq!(replayed["duration"], 8);
        assert!(replayed.get("scenario").is_none());

        assert!(scenario(folder.path(), &json!({ "scenario": "../secret" })).is_err());
        assert!(scenario(folder.path(), &json!({ "scenario": "absent" })).unwrap_err().contains("introuvable"));
    }

    #[test]
    fn only_actions_on_the_live_session_need_to_drive_studio() {
        assert!(drives_studio("playtest_check", &json!({})));
        assert!(drives_studio("screenshot", &json!({})));
        assert!(drives_studio("run_luau", &json!({ "context": "client" })));
        assert!(!drives_studio("run_luau", &json!({ "code": "print(1)" })));
        assert!(!drives_studio("get_tree", &json!({})));
        assert!(!drives_studio("get_console", &json!({})));
    }

    #[test]
    fn a_verdict_names_failures_before_doubts() {
        let mut report = Report::default();
        assert!(report.verdict().starts_with("PASS"));
        report.doubts.push("build".into());
        assert!(report.verdict().starts_with("INCONCLUSIVE — build"));
        report.failures.push("assert_luau a affiché FAIL".into());
        assert!(report.verdict().starts_with("FAIL — assert_luau"));
    }

    /// The UI's refresh and an agent's studio_status read the same things.
    /// Each once took two locks in the opposite order, and the server froze
    /// the day both ran at the same instant.
    #[test]
    fn a_refresh_and_a_status_call_never_wait_on_each_other() {
        let state: Shared = Arc::new(AppState::new(0, "t".into(), std::env::temp_dir(), "0".into()));
        let (done, finished) = std::sync::mpsc::channel();

        let refreshing = state.clone();
        let refresh_done = done.clone();
        std::thread::spawn(move || {
            let runtime = tokio::runtime::Builder::new_current_thread().build().unwrap();
            for _ in 0..150 {
                let _ = runtime.block_on(crate::api::get_state(axum::extract::State(refreshing.clone())));
            }
            let _ = refresh_done.send(());
        });
        std::thread::spawn(move || {
            let project = Project {
                id: "p".into(),
                name: "Jeu".into(),
                path: std::env::temp_dir(),
                sync_port: 1,
                place_id: None,
                place_name: None,
                protected: false,
            };
            for _ in 0..150 {
                studio_status(&state, Some(&project));
            }
            let _ = done.send(());
        });

        for _ in 0..2 {
            finished
                .recv_timeout(Duration::from_secs(60))
                .expect("les deux lectures se sont bloquées l'une l'autre");
        }
    }
}
