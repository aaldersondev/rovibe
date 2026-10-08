# Origine

Ce dossier est une copie de [rojo-rbx/rojo](https://github.com/rojo-rbx/rojo) au commit `7e60406373faf82887353164543540d054d84d92` (7.7.1), avec ses sous-modules, modifiée pour Essaim.

Fichiers modifiés par rapport à l'amont :

- `plugin/src/Bridge.lua` (ajouté) : la connexion WebSocket vers l'app.
- `plugin/src/init.server.lua`, `plugin/src/App/init.lua` : démarrage du pont, connexion de la synchro pilotée par l'app, nom affiché.
- `src/cli/plugin.rs` : nom du fichier du plugin installé.

Licence : MPL-2.0, voir `LICENSE.txt`. Ces fichiers restent sous cette licence.
