// The interface is written in French; this is its English.
//
// Text is translated where it reaches the page, by looking the French up
// here, so the rest of the code builds its sentences once. A sentence with
// values in it is listed with `{}` where each value goes. What the server
// writes itself (tool results, the journal) is not covered and stays French.

const saved = localStorage.getItem("rovibe.lang");
/** `fr` or `en`: the user's choice, or the language of the system. */
export const lang: "fr" | "en" =
  saved === "fr" || saved === "en" ? saved : navigator.language.toLowerCase().startsWith("fr") ? "fr" : "en";

const EXACT: Record<string, string> = {
  // Panes
  "Reçoit les consignes envoyées à plusieurs agents": "Receives the prompts sent to several agents",
  "Glisse pour changer l'ordre des panneaux": "Drag to reorder the panes",
  "branche à part": "own branch",
  "Intègre le travail de cet agent au projet. Ce qu'il n'a pas commité l'est d'abord ; en cas de conflit, rien n'est modifié.":
    "Brings this agent's work into the project. Anything it left uncommitted is committed first; on a conflict, nothing changes.",
  Fusionner: "Merge",
  "Agrandir ou réduire": "Maximize or restore",
  "Fermer la session": "Close the session",
  démarre: "starting",
  travaille: "working",
  "attend ta réponse": "waiting for you",
  "a fini": "done",
  terminée: "ended",
  isolé: "isolated",
  isolée: "isolated",
  prêt: "ready",
  commande: "command",
  "une question t'attend dans son terminal": "a question is waiting in its terminal",
  "1 fichier tenu": "1 file held",

  // Project dialog
  "Aucune place ouverte dans Studio": "No place open in Studio",
  "Ne rien importer : projet vide": "Import nothing: empty project",
  "Nouveau projet": "New project",
  "Nom du jeu": "Game name",
  "Dossier (facultatif). Un dossier qui contient déjà un default.project.json est repris tel quel.":
    "Folder (optional). A folder that already holds a default.project.json is used as it is.",
  "Partir d'un jeu existant. Seuls les scripts deviennent des fichiers ; la place n'est pas modifiée.":
    "Start from an existing game. Only scripts become files; the place is left untouched.",
  Annuler: "Cancel",
  "Créer le projet": "Create the project",
  "Documents\\RoVibe\\<nom>": "Documents\\RoVibe\\<name>",

  // History and changes
  Historique: "History",
  "Ce projet n'a pas son propre dépôt git : RoVibe ne gère pas son historique.":
    "This project has no git repository of its own: RoVibe doesn't keep its history.",
  Fermer: "Close",
  "Nom du point de sauvegarde (facultatif)": "Checkpoint name (optional)",
  "Aucun changement depuis le dernier point de sauvegarde.": "No change since the last checkpoint.",
  "Créer un point de sauvegarde": "Create a checkpoint",
  "État actuel": "Current state",
  "Remet les fichiers dans cet état. L'état actuel est d'abord sauvegardé, rien n'est perdu.":
    "Puts the files back in this state. The current state is saved first, nothing is lost.",
  "Revenir ici": "Go back here",
  modifié: "modified",
  supprimé: "deleted",
  "auteur inconnu (session fermée ou modification à la main)": "unknown author (closed session or manual edit)",
  "Remet ce fichier dans l'état de ta dernière relecture": "Puts this file back as it was at your last review",
  "Supprimer ce nouveau fichier": "Delete this new file",
  Changements: "Changes",
  "Rien n'a changé depuis ta dernière relecture.": "Nothing changed since your last review.",
  "Crée un point de sauvegarde et repart de cet état pour la prochaine relecture":
    "Creates a checkpoint and starts the next review from this state",
  "Tout accepter": "Accept all",

  // Settings
  "Celui de Claude Code (ex. opus, sonnet, haiku)": "Claude Code's own (e.g. opus, sonnet, haiku)",
  "Celui de Codex": "Codex's own",
  "Restreint : l'API du modèle et les hôtes ci-dessous": "Restricted: the model's API and the hosts below",
  "Ouvert : tout internet": "Open: the whole internet",
  "Quand un agent m'attend ou a fini, si la fenêtre n'est pas devant":
    "When an agent needs me or is done, if the window isn't in front",
  "Seulement quand un agent m'attend": "Only when an agent needs me",
  Jamais: "Never",
  "Me demander": "Ask me",
  "Continuer en arrière-plan": "Keep running in the background",
  "Arrêter les agents et quitter": "Stop the agents and quit",
  Réglages: "Settings",
  "Modèle par défaut des sessions Claude Code": "Default model of Claude Code sessions",
  "Modèle par défaut des sessions Codex": "Default model of Codex sessions",
  "Dossier où créer les nouveaux projets (chemin complet)": "Folder for new projects (full path)",
  "Raccourci « Publier sur Roblox » de Studio, si tu l'as changé. Touches séparées par +.":
    "Studio's “Publish to Roblox” shortcut, if you changed it. Keys separated by +.",
  "Réseau des agents isolés (Claude Code dans WSL)": "Network of isolated agents (in WSL)",
  "Hôtes en plus de l'API du modèle, en HTTPS. Un nom, ou *.domaine pour tout un domaine.":
    "Hosts besides the model's API, over HTTPS. A name, or *.domain for a whole domain.",
  "ex. github.com *.githubusercontent.com": "e.g. github.com *.githubusercontent.com",
  "Fermer la fenêtre pendant que des sessions tournent": "Closing the window while sessions run",
  "Notifications Windows": "Windows notifications",
  Langue: "Language",
  "Celle du système": "The system's",
  Enregistrer: "Save",

  // Settings page
  Enregistré: "Saved",
  Retour: "Back",
  "Revenir au projet": "Back to the project",
  Sections: "Sections",
  Général: "General",
  "Ce qui tient à l'app elle-même.": "What belongs to the app itself.",
  "L'interface suit la langue du système, ou celle que tu choisis ici.": "The interface follows the system's language, or the one you choose here.",
  "En arrière-plan, les agents continuent et l'icône près de l'horloge rouvre la fenêtre.":
    "In the background, agents carry on and the icon by the clock brings the window back.",
  "Elles ne partent que si la fenêtre n'est pas devant.": "They are only sent when the window isn't in front.",
  "Quand un agent m'attend ou a fini": "When an agent needs me or is done",
  "Dossier des nouveaux projets": "Folder for new projects",
  "Chemin complet. Vide : Documents\\RoVibe.": "Full path. Empty: Documents\\RoVibe.",
  Agents: "Agents",
  "Les modèles donnés aux nouvelles sessions. Celui de Claude Code se choisit aussi au lancement.":
    "The models given to new sessions. Claude Code's can also be chosen when starting one.",
  "Modèle de Claude Code": "Claude Code's model",
  "Vide : celui que Claude Code choisit lui-même.": "Empty: the one Claude Code picks itself.",
  "ex. opus, sonnet, haiku": "e.g. opus, sonnet, haiku",
  "Modèle de Codex": "Codex's model",
  "Vide : celui que Codex choisit lui-même.": "Empty: the one Codex picks itself.",
  "Le lien entre l'app et Studio.": "The link between the app and Studio.",
  "Plugin RoVibe Studio": "RoVibe Studio plugin",
  "Après une installation ou une mise à jour, redémarre Studio pour le charger.": "After installing or updating it, restart Studio to load it.",
  "Raccourci « Publier sur Roblox »": "“Publish to Roblox” shortcut",
  "À changer seulement si tu l'as changé dans Studio. Touches séparées par +.": "Change it only if you changed it in Studio. Keys separated by +.",
  "Un agent lancé avec « Isolé » tourne dans une distribution WSL où il ne voit que son projet.":
    "An agent started with “Isolated” runs in a WSL distribution where it only sees its project.",
  "Distribution WSL": "WSL distribution",
  "Créée une fois par scripts\\setup-isolation.ps1.": "Created once by scripts\\setup-isolation.ps1.",
  Réseau: "Network",
  "Restreint, un agent isolé ne joint que l'API de son modèle et les hôtes ci-dessous.":
    "Restricted, an isolated agent only reaches its model's API and the hosts below.",
  Restreint: "Restricted",
  "Hôtes autorisés en plus": "Extra allowed hosts",
  "En HTTPS. Un nom par hôte, ou *.domaine pour tout un domaine.": "Over HTTPS. One name per host, or *.domain for a whole domain.",
  "À propos": "About",
  "Mise à jour": "Update",
  "L'app en cherche une à chaque démarrage.": "The app looks for one each time it starts.",
  Diagnostic: "Diagnostics",
  "Ce que l'app a fait, et ce dont elle a besoin sur ce PC.": "What the app did, and what it needs on this PC.",
  "Code source": "Source code",
  Installé: "Installed",
  Installée: "Installed",
  "Introuvable dans le PATH": "Not found in the PATH",
  "Pas installé": "Not installed",
  "Pas installée": "Not installed",
  Réinstaller: "Reinstall",
  Connecté: "Connected",
  "Non connecté": "Not connected",
  "Aucune mise à jour en attente": "No update waiting",
  "Langue, modèles, Studio, agents isolés": "Language, models, Studio, isolated agents",

  // Journal
  "Le journal est vide.": "The journal is empty.",
  Journal: "Journal",
  "Ce que l'app a fait, du plus ancien au plus récent : sessions, synchro, connexions de Studio, outils en erreur, mises à jour.":
    "What the app did, oldest first: sessions, sync, Studio connections, failed tools, updates.",

  // Asset bank
  "Filtrer par nom, tag ou collection": "Filter by name, tag or collection",
  "Sans collection": "No collection",
  "Collection de cet asset": "This asset's collection",
  "fichier ajouté à la main": "file added by hand",
  "Supprimer de la banque": "Remove from the bank",
  Supprimer: "Delete",
  "La banque est vide. Demande à un agent « enregistre Workspace.MonModele dans la banque », importe un pack, ou dépose des fichiers .rbxm dans le dossier ci-dessous.":
    "The bank is empty. Ask an agent to “save Workspace.MyModel to the bank”, import a pack, or drop .rbxm files in the folder below.",
  "Aucun asset ne correspond.": "No asset matches.",
  "Chaque asset sans image est posé un instant dans la place ouverte dans Studio, photographié, puis retiré":
    "Each asset without a picture is placed for a moment in the place open in Studio, pictured, then removed",
  "Aperçus en cours…": "Making previews…",
  "Créer les aperçus manquants": "Make the missing previews",
  "Dossier du pack, ex. C:\\Packs\\Nature": "Pack folder, e.g. C:\\Packs\\Nature",
  "Collection (facultatif)": "Collection (optional)",
  "Importer le pack": "Import the pack",
  "Chercher dans le Creator Store (gratuits)": "Search the Creator Store (free assets)",
  Modèles: "Models",
  Sons: "Sounds",
  "Tape une recherche.": "Type a search.",
  "Recherche…": "Searching…",
  "Contient des scripts": "Contains scripts",
  "Sans script": "No script",
  "Pose l'asset dans Workspace, scripts désactivés": "Puts the asset in Workspace, scripts disabled",
  Inséré: "Inserted",
  "Insérer dans Studio": "Insert in Studio",
  "Aucun asset gratuit ne correspond.": "No free asset matches.",
  Chercher: "Search",
  "Les scripts d'un asset du Store sont désactivés à l'insertion : un modèle gratuit peut cacher une porte dérobée.":
    "Scripts of a Store asset are disabled on insertion: a free model can hide a backdoor.",
  "Ma banque": "My bank",
  "Banque d'assets": "Asset bank",
  "Modèles réutilisables d'un projet à l'autre. Les agents les trouvent avec asset_search, les regardent avec asset_preview et les posent avec asset_insert, tout comme les assets gratuits du Creator Store.":
    "Models to reuse from one project to the next. Agents find them with asset_search, look at them with asset_preview and place them with asset_insert, like the Creator Store's free assets.",

  // Command bar
  Consigne: "Prompt",
  "Écris une consigne pour les agents sélectionnés…": "Write a prompt for the selected agents…",
  "Consignes enregistrées dans ce projet": "Prompts saved in this project",
  "Aucune consigne enregistrée": "No saved prompt",
  "Consignes enregistrées": "Saved prompts",
  Pour: "To",
  "Ne plus viser personne": "Select nobody",
  "Viser toutes les sessions": "Select every session",
  Tous: "All",
  Entrée: "Enter",
  Maj: "Shift",
  envoyer: "send",
  "nouvelle ligne": "new line",
  "Supprimer cette consigne enregistrée": "Delete this saved prompt",
  "Garder cette consigne dans le projet pour la réutiliser": "Keep this prompt in the project to reuse it",
  "Nom de la consigne": "Name of the prompt",
  "Aucun destinataire": "No recipient",
  "Envoyer à 1 agent": "Send to 1 agent",

  // Closing, updates, requests
  "1 session est en cours": "1 session is running",
  "RoVibe peut continuer en arrière-plan : les agents poursuivent leur travail, et l'icône près de l'horloge rouvre la fenêtre. Quitter les arrête ; leurs conversations seront proposées à la reprise.":
    "RoVibe can keep running in the background: agents carry on, and the icon by the clock brings the window back. Quitting stops them; their conversations will be offered again.",
  "Ne plus me demander": "Don't ask again",
  Quitter: "Quit",
  "Installer et redémarrer": "Install and restart",
  Autoriser: "Allow",
  Refuser: "Refuse",

  // Rail
  Projets: "Projects",
  "1 agent attend ta réponse": "1 agent is waiting for you",
  "1 session à reprendre": "1 session to resume",
  "Aucun agent": "No agent",
  "1 agent actif": "1 agent running",
  Outils: "Tools",
  "Modèles réutilisables et Creator Store": "Reusable models and Creator Store",
  "Plugin Studio": "Studio plugin",
  "Copie RoVibeStudio.rbxm dans le dossier Plugins de Roblox Studio": "Copies RoVibeStudio.rbxm to Roblox Studio's Plugins folder",
  "mettre à jour": "update",
  "à installer": "to install",
  "Modèles par défaut, dossiers, réseau des agents isolés": "Default models, folders, network of isolated agents",
  "Ce que l'app a fait": "What the app did",
  "Premiers pas": "Getting started",
  "Ce dont RoVibe a besoin sur ce PC, et ce qui manque": "What RoVibe needs on this PC, and what is missing",
  "1 à régler": "1 to fix",

  // Setup checklist
  "Tout ce qu'il faut est en place. Le reste est facultatif.": "Everything required is in place. The rest is optional.",
  "Voici ce qu'il reste à mettre en place pour que les agents puissent travailler sur ton jeu.":
    "Here is what still has to be set up before agents can work on your game.",
  "Installe-le avec « npm install -g @anthropic-ai/claude-code », connecte-toi une fois en lançant « claude » dans un terminal, puis relance RoVibe.":
    "Install it with “npm install -g @anthropic-ai/claude-code”, sign in once by running “claude” in a terminal, then restart RoVibe.",
  "Facultatif. Installe-le avec « npm install -g @openai/codex », puis relance RoVibe.":
    "Optional. Install it with “npm install -g @openai/codex”, then restart RoVibe.",
  "Sert à l'historique, à la relecture des changements et aux branches à part. À installer depuis git-scm.com.":
    "Needed for history, change review and own branches. Install it from git-scm.com.",
  "Moteur de synchro": "Sync engine",
  "rovibe-sync.exe manque à côté de l'app : réinstalle RoVibe, ou compile vendor/sync.":
    "rovibe-sync.exe is missing next to the app: reinstall RoVibe, or build vendor/sync.",
  "Plugin Roblox Studio": "Roblox Studio plugin",
  "Le plugin relie Studio à RoVibe. Installe-le, puis redémarre Studio.":
    "The plugin links Studio to RoVibe. Install it, then restart Studio.",
  "Installer le plugin": "Install the plugin",
  "Roblox Studio connecté": "Roblox Studio connected",
  "Ouvre une place dans Studio. S'il était ouvert pendant l'installation du plugin, redémarre-le.":
    "Open a place in Studio. If it was open while the plugin was installed, restart it.",
  "Vérification du code": "Code checking",
  "Facultatif : selene et luau-lsp relisent le code des agents. Lance scripts\\get-tools.ps1 pour les installer.":
    "Optional: selene and luau-lsp check the agents' code. Run scripts\\get-tools.ps1 to install them.",
  "Agents isolés": "Isolated agents",
  "Facultatif : une distribution WSL où un agent ne voit que son projet. Lance scripts\\setup-isolation.ps1 une fois.":
    "Optional: a WSL distribution where an agent only sees its project. Run scripts\\setup-isolation.ps1 once.",
  "Créer mon premier projet": "Create my first project",

  // Bar
  "Studio non connecté": "Studio not connected",
  "Place Studio visée par les agents de ce projet": "Studio place this project's agents work on",
  "Place : automatique": "Place: automatic",
  "Place Studio vue par les agents de ce projet": "Studio place seen by this project's agents",
  "Arrêter le serveur de synchro": "Stop the sync server",
  "Démarrer le serveur de synchro": "Start the sync server",
  "Synchro arrêtée": "Sync stopped",
  "Synchro : branche d'un agent": "Sync: an agent's branch",
  "Studio montre la branche d'un agent : reconnecter le ramène au dossier du projet":
    "Studio shows an agent's branch: connecting again brings it back to the project folder",
  "Connecte Studio au serveur de synchro du projet": "Connects Studio to the project's sync server",
  Connecter: "Connect",
  "Projet protégé : Studio montre les changements avant chaque synchro, et un agent doit te demander avant de la connecter. Clique pour retirer la protection.":
    "Protected project: Studio shows the changes before every sync, and an agent must ask you before connecting it. Click to remove the protection.",
  "Protéger ce projet : Studio demandera avant chaque synchro, et un agent devra te demander avant de la connecter. À activer pour un jeu en ligne.":
    "Protect this project: Studio will ask before every sync, and an agent will have to ask you before connecting it. Turn it on for a live game.",
  Protégé: "Protected",
  Protéger: "Protect",
  Projet: "Project",
  "Ce que les agents ont modifié depuis ta dernière relecture": "What agents changed since your last review",
  "Points de sauvegarde et retour en arrière": "Checkpoints and going back",
  "Met la place ouverte dans Studio en ligne sur Roblox, par le raccourci de publication de Studio":
    "Puts the place open in Studio live on Roblox, through Studio's publish shortcut",
  Publier: "Publish",
  Lancer: "Start",
  avec: "with",
  "Nouvelle session Claude Code": "New Claude Code session",
  "claude introuvable dans le PATH": "claude not found in the PATH",
  "Nouvelle session Codex": "New Codex session",
  "codex introuvable dans le PATH": "codex not found in the PATH",
  "Un terminal dans le dossier du projet": "A terminal in the project folder",
  "Modèle de la prochaine session Claude Code": "Model of the next Claude Code session",
  "Modèle des réglages": "Model from settings",
  "Les agents agissent sans demander de confirmation. Un point de sauvegarde est créé avant chaque session lancée ainsi.":
    "Agents act without asking for confirmation. A checkpoint is created before each session started this way.",
  "Sans confirmations": "No confirmations",
  "L'agent ne voit que le dossier du projet. Il tourne dans une distribution WSL sans accès au reste du PC ; Codex garde son propre bac à sable tant qu'il n'y est pas installé.":
    "The agent only sees the project folder. It runs in a WSL distribution with no access to the rest of the PC; Codex keeps its own sandbox until it is installed there.",
  "Codex seulement pour l'instant, dans son propre bac à sable. Pour la distribution WSL, lance scripts\\setup-isolation.ps1 une fois.":
    "Codex only for now, in its own sandbox. For the WSL distribution, run scripts\\setup-isolation.ps1 once.",
  Isolé: "Isolated",
  "Chaque nouvel agent travaille dans une copie du projet, sur sa propre branche git : plusieurs agents peuvent alors modifier les mêmes fichiers. Tu fusionnes leur travail depuis leur panneau.":
    "Each new agent works in a copy of the project, on its own git branch: several agents can then change the same files. You merge their work from their pane.",
  "Branche à part": "Own branch",
  "Disposition des panneaux": "Pane layout",
  "Tous les panneaux côte à côte": "All panes side by side",
  "Un seul panneau à la fois, en grand": "One pane at a time, full size",

  // Resume strip and empty states
  "Session interrompue": "Interrupted session",
  "Sessions interrompues": "Interrupted sessions",
  "Ouverte à la dernière fermeture de l'app. La reprendre relance l'agent sur sa conversation.":
    "Open when the app last closed. Resuming restarts the agent on its conversation.",
  Reprendre: "Resume",
  "Oublier cette session": "Forget this session",
  "Tout reprendre": "Resume all",
  "Un projet est un dossier de code synchronisé avec une place Roblox Studio. Les agents y travaillent en parallèle.":
    "A project is a folder of code synced with a Roblox Studio place. Agents work in it side by side.",
  "Aucun agent sur ce projet": "No agent on this project",
  "Lance un ou plusieurs agents. Chacun reçoit les outils Studio du projet et travaille dans son dossier.":
    "Start one or more agents. Each gets the project's Studio tools and works in its folder.",
  "Lancer Claude Code": "Start Claude Code",
  "Ouvrir un terminal": "Open a terminal",
};

/** Sentences with values: `{}` stands for each value, in order. */
const WITH_VALUES: [string, string][] = [
  ["Erreur {}", "Error {}"],
  ["Installer RoVibe {}", "Install RoVibe {}"],
  [
    "Travaille dans son propre dossier, sur la branche {}. Studio montre le projet tant que l'agent n'a pas connecté la synchro à sa branche.",
    "Works in its own folder, on branch {}. Studio shows the project until the agent connects the sync to its branch.",
  ],
  ["Arrêter « {} » ?", "Stop “{}”?"],
  ["Importer les scripts de « {} »", "Import the scripts of “{}”"],
  ["{} fichier(s) modifié(s) depuis le dernier point de sauvegarde.", "{} file(s) changed since the last checkpoint."],
  ["Revenir à « {} » ? L'état actuel sera sauvegardé avant.", "Go back to “{}”? The current state will be saved first."],
  ["Annuler les changements de {} ?", "Undo the changes to {}?"],
  ["Supprimer ce nouveau fichier {} ?", "Delete this new file {}?"],
  [
    "{} fichier(s) modifié(s) depuis ta dernière relecture, commités ou non par les agents.",
    "{} file(s) changed since your last review, committed by the agents or not.",
  ],
  ["Fichier : {}", "File: {}"],
  ["Toutes les collections ({})", "All collections ({})"],
  ["Supprimer « {} » de la banque ?", "Remove “{}” from the bank?"],
  ["{} aperçu(s) créé(s), {} impossible(s)", "{} preview(s) made, {} failed"],
  ["{} aperçu(s) créé(s)", "{} preview(s) made"],
  ["{} asset(s) importé(s)", "{} asset(s) imported"],
  [
    "Un pack est un dossier de fichiers .rbxm ou .rbxmx ; ses sous-dossiers deviennent des collections. Dossier de la banque : {}",
    "A pack is a folder of .rbxm or .rbxmx files; its sub-folders become collections. Bank folder: {}",
  ],
  ["Consigne envoyée à {} agents", "Prompt sent to {} agents"],
  ["Consigne envoyée à {}", "Prompt sent to {}"],
  ["Supprimer la consigne « {} » ?", "Delete the prompt “{}”?"],
  ["Envoyer aux {} agents", "Send to {} agents"],
  ["{} sessions sont en cours", "{} sessions are running"],
  [
    "RoVibe {} est disponible. L'installer redémarre l'app ; tes sessions Claude Code seront proposées à la reprise.",
    "RoVibe {} is available. Installing it restarts the app; your Claude Code sessions will be offered again.",
  ],
  ["{} agents attendent ta réponse", "{} agents are waiting for you"],
  ["{} sessions à reprendre", "{} sessions to resume"],
  ["{} agents actifs", "{} agents running"],
  ["{} à régler", "{} to fix"],
  ["{} : bloqué", "{}: blocked"],
  ["Place : {} (fermée)", "Place: {} (closed)"],
  ["Place : {}", "Place: {}"],
  [
    "Studio attend une réponse : {}. Tant qu'elle est ouverte, il ignore les touches, les clics et les tests.",
    "Studio is waiting for an answer: {}. While it is open, it ignores keys, clicks and tests.",
  ],
  ["Synchro : port {}", "Sync: port {}"],
  ["Publier « {} » sur Roblox ? Les joueurs recevront cette version.", "Publish “{}” to Roblox? Players will get this version."],
  ["Serveur injoignable : {}", "Server unreachable: {}"],
  ["{} fichiers tenus", "{} files held"],
  ["{} agents", "{} agents"],
  ["{} au travail", "{} working"],
  ["{} en attente de toi", "{} waiting for you"],
  ["modifie {}", "editing {}"],
  ["demande l'autorisation d'utiliser {}", "asks permission to use {}"],
  ["{} demande à {}.", "{} asks to {}."],
];

const PATTERNS: [RegExp, string][] = WITH_VALUES.map(([french, english]) => [
  new RegExp(`^${french.replace(/[.*+?^$()|[\]\\]/g, "\\$&").replace(/\{\}/g, "(.+?)")}$`, "s"),
  english,
]);

/** The text in the language of the interface. Unknown text comes back as it
 *  is: a missing translation shows French, never nothing. */
export function tr(text: string): string {
  if (lang === "fr") return text;
  const exact = EXACT[text];
  if (exact !== undefined) return exact;
  for (const [pattern, english] of PATTERNS) {
    const found = pattern.exec(text);
    if (found) {
      let index = 0;
      return english.replace(/\{\}/g, () => tr(found[++index] ?? ""));
    }
  }
  return text;
}
