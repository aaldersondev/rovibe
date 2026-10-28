# Origine

Ce dossier est une copie de [rojo-rbx/rojo](https://github.com/rojo-rbx/rojo) au commit `7e60406373faf82887353164543540d054d84d92` (7.7.1), avec ses sous-modules, modifiée pour RoVibe.

Fichiers modifiés par rapport à l'amont :

- `plugin/src/Bridge.lua` (ajouté) : la connexion WebSocket vers l'app, qui annonce le projet dont la place porte le marqueur.
- `plugin/src/PlaceMarker.lua` (ajouté) : le marqueur `ServerStorage.RoVibe` (attribut `ProjectId`) qui dit à quel projet RoVibe appartient une place.
- `plugin/src/init.server.lua` : dans les DataModels d'un playtest, seul le pont démarre (ni Roact ni le panneau).
- `plugin/src/App/init.lua` : connexion de la synchro pilotée par l'app avec son jeton, état de la synchro pour l'app, nom affiché.
- `plugin/src/ApiContext.lua` : jeton `X-RoVibe-Token` sur chaque requête et sur le WebSocket, refus d'un serveur lancé pour un autre projet que celui de la place, test de reprise du flux.
- `plugin/src/ServeSession.lua` : un flux de messages coupé est rouvert tant que le serveur a encore tout l'historique depuis le curseur, au lieu de terminer la session et de tout resynchroniser.
- `plugin/src/Types.lua`, `plugin/http/init.lua`, `plugin/http/Error.lua` : nouveaux champs de `/api/rojo`, en-têtes HTTP, message d'un refus de jeton.
- `src/message_queue.rs` : historique borné à 4096 messages (l'amont gardait tout), avec le plus ancien curseur encore disponible.
- `src/serve_session.rs`, `src/web/api.rs`, `src/web/interface.rs` : `messageCursor`, `oldestMessageCursor` et `rovibeProjectId` (variable `ROVIBE_PROJECT_ID`) dans `/api/rojo` ; `/api/rovibe/status` en JSON (avec le nombre de Studios connectés) ; un curseur plus vieux que l'historique est refusé (410).
- `src/web/token.rs` (ajouté), `src/web/mod.rs` : si `ROVIBE_SYNC_TOKEN` est défini, toute requête `/api` doit porter ce jeton.
- `rojo-test/serve-test-snapshots` : les deux nouveaux champs de `/api/rojo`.
- `src/cli/plugin.rs` : nom du fichier du plugin installé.

Licence : MPL-2.0, voir `LICENSE.txt`. Ces fichiers restent sous cette licence.
