import "@fontsource-variable/bricolage-grotesque";
import "@xterm/xterm/css/xterm.css";
import "./style.css";

import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";

import { lang, tr } from "./i18n";

document.documentElement.lang = lang;

type Kind = "claude" | "codex" | "shell";

interface Project {
  id: string;
  name: string;
  path: string;
  sync_port: number;
  place_id: number | null;
  place_name: string | null;
  sync_running: boolean;
  /** Studio asks before every sync, and agents ask before connecting it. */
  protected: boolean;
  /** Studio is being shown an agent's branch instead of the project. */
  sync_branch: boolean;
}

interface Session {
  id: string;
  project_id: string;
  kind: Kind;
  title: string;
  exited: boolean;
  /** "starting", "working", "waiting", "idle", or "" when the agent doesn't report. */
  status: string;
  detail: string;
  since: number;
  files: string[];
  isolated: boolean;
  /** Its own git branch, when it works in a folder of its own. */
  branch?: string;
}

interface Studio {
  id: number;
  place_id: number;
  name: string;
  context: string;
  /** Names the dialog that blocks this Studio, when one is open. */
  blocked?: string | null;
}

/** A session left open by the previous run, waiting to be resumed. */
interface Dormant {
  agent_id: string;
  project_id: string;
  title: string;
  isolated: boolean;
}

interface State {
  version: string;
  projects: Project[];
  sessions: Session[];
  studios: Studio[];
  tools: { claude: boolean; codex: boolean; git: boolean; sync: boolean };
  checkers: { selene: boolean; luau_lsp: boolean };
  approvals: { id: number; project_id: string; requester: string; request: string }[];
  /** Version of an update waiting to be installed, if any. */
  update: string | null;
  /** Version already installed, which this older server keeps from showing
   *  because sessions still run on it. */
  pending: string | null;
  dormant: Dormant[];
  /** Whether the WSL distribution for isolated Claude Code sessions exists. */
  isolation: boolean;
  plugin_installed: boolean;
}

interface Pane {
  /** Opens the terminal; only valid once `element` is in the document. */
  mount: () => void;
  element: HTMLElement;
  terminal: Terminal;
  socket: WebSocket;
  state: HTMLElement;
  files: HTMLElement;
  /** Whether a broadcast prompt goes to this session. */
  target: HTMLInputElement;
}

const token = document.querySelector<HTMLMetaElement>('meta[name="rovibe-token"]')!.content;
const app = document.getElementById("app")!;
const panes = new Map<string, Pane>();

let state: State | null = null;
let selected = localStorage.getItem("rovibe.project");
let skipPermissions = localStorage.getItem("rovibe.skip") === "1";
let isolated = localStorage.getItem("rovibe.isolated") === "1";
/** New agents get a folder and a git branch of their own. */
let apart = localStorage.getItem("rovibe.apart") === "1";
/** Model for the next Claude Code session; empty follows the settings. */
let model = localStorage.getItem("rovibe.model") ?? "";

type Child = Node | string | null | false;

/** Attributes a person reads, as opposed to those the page works with. */
const SPOKEN = new Set(["title", "placeholder", "aria-label"]);

function h<K extends keyof HTMLElementTagNameMap>(
  tag: K,
  props: Record<string, unknown> = {},
  ...children: Child[]
): HTMLElementTagNameMap[K] {
  const element = document.createElement(tag);
  for (const [key, value] of Object.entries(props)) {
    if (key.startsWith("on") && typeof value === "function") {
      element.addEventListener(key.slice(2), value as EventListener);
    } else if (value === true) {
      element.setAttribute(key, "");
    } else if (value !== false && value != null) {
      element.setAttribute(key, SPOKEN.has(key) ? tr(String(value)) : String(value));
    }
  }
  for (const child of children) {
    if (child) element.append(typeof child === "string" ? tr(child) : child);
  }
  return element;
}

/** Line icons, drawn on a 24-unit grid with the text color. */
const ICONS = {
  plus: "M5 12h14M12 5v14",
  close: "M18 6 6 18M6 6l12 12",
  expand: "M15 3h6v6M9 21H3v-6M21 3l-7 7M3 21l7-7",
  folder: "M20 20a2 2 0 0 0 2-2V8a2 2 0 0 0-2-2h-7.9a2 2 0 0 1-1.69-.9L9.6 3.9A2 2 0 0 0 7.93 3H4a2 2 0 0 0-2 2v13a2 2 0 0 0 2 2Z",
  link: "M10 13a5 5 0 0 0 7.54.54l3-3a5 5 0 0 0-7.07-7.07l-1.72 1.71M14 11a5 5 0 0 0-7.54-.54l-3 3a5 5 0 0 0 7.07 7.07l1.71-1.71",
  changes: "M3 12a9 9 0 1 0 9-9 9.75 9.75 0 0 0-6.74 2.74L3 8M3 3v5h5M12 7v5l4 2",
  history: "M15 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V7ZM14 2v4a2 2 0 0 0 2 2h4M10 9H8M16 13H8M16 17H8",
  publish: "M12 13v8M4 14.899A7 7 0 1 1 15.71 8h1.79a4.5 4.5 0 0 1 2.5 8.242M8 17l4-4 4 4",
  claude: "M12 3v18M3 12h18M5.6 5.6l12.8 12.8M18.4 5.6 5.6 18.4",
  codex: "M21 16V8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16zM8.5 10l3.5 2-3.5 2M13 14h3",
  shell: "M5 3h14a2 2 0 0 1 2 2v14a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2zM7 9l3 3-3 3M12 15h5",
  bank: "M21 8a2 2 0 0 0-1-1.73l-7-4a2 2 0 0 0-2 0l-7 4A2 2 0 0 0 3 8v8a2 2 0 0 0 1 1.73l7 4a2 2 0 0 0 2 0l7-4A2 2 0 0 0 21 16ZM3.3 7l8.7 5 8.7-5M12 22V12",
  plugin: "M12 22v-5M9 8V2M15 8V2M18 8v5a4 4 0 0 1-4 4h-4a4 4 0 0 1-4-4V8Z",
  settings: "M21 4h-7M10 4H3M21 12h-9M8 12H3M21 20h-5M12 20H3M14 2v4M8 10v4M16 18v4",
  log: "M8 6h13M8 12h13M8 18h13M3 6h.01M3 12h.01M3 18h.01",
  save: "M15.2 3a2 2 0 0 1 1.4.6l3.8 3.8a2 2 0 0 1 .6 1.4V19a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2zM17 21v-7a1 1 0 0 0-1-1H8a1 1 0 0 0-1 1v7M7 3v4a1 1 0 0 0 1 1h7",
  branch: "M6 3v12M18 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM6 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM18 9a9 9 0 0 1-9 9",
  merge: "M18 21a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM6 9a3 3 0 1 0 0-6 3 3 0 0 0 0 6zM6 21V9a9 9 0 0 0 9 9",
  grid: "M3 3h7v7H3zM14 3h7v7h-7zM14 14h7v7h-7zM3 14h7v7H3z",
  tabs: "M3 8h18v12H3zM3 8V4h8v4",
  back: "M19 12H5M12 19l-7-7 7-7",
  check: "M20 6 9 17l-5-5",
  alert: "M12 9v4M12 17h.01M10.3 3.9 1.8 18a2 2 0 0 0 1.7 3h17a2 2 0 0 0 1.7-3L13.7 3.9a2 2 0 0 0-3.4 0z",
  setup: "M9 11l3 3L22 4M21 12v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V5a2 2 0 0 1 2-2h11",
  lock: "M7 11V7a5 5 0 0 1 10 0v4M5 11h14a2 2 0 0 1 2 2v7a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-7a2 2 0 0 1 2-2z",
  send: "M14.536 21.686a.5.5 0 0 0 .937-.024l6.5-19a.496.496 0 0 0-.635-.635l-19 6.5a.5.5 0 0 0-.024.937l7.93 3.18a2 2 0 0 1 1.112 1.11zM21.854 2.147l-10.94 10.939",
} as const;

function icon(name: keyof typeof ICONS) {
  const svg = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  svg.setAttribute("viewBox", "0 0 24 24");
  // Prefixed: several icon names are also the names of parts of the page.
  svg.setAttribute("class", `icon i-${name}`);
  svg.setAttribute("aria-hidden", "true");
  const shape = document.createElementNS("http://www.w3.org/2000/svg", "path");
  shape.setAttribute("d", ICONS[name]);
  svg.append(shape);
  return svg;
}

async function api<T = { message?: string }>(path: string, method = "GET", body?: unknown): Promise<T> {
  const response = await fetch(path, {
    method,
    headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
    body: body === undefined ? undefined : JSON.stringify(body),
  });
  const data = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(data.error ?? `Erreur ${response.status}`);
  return data as T;
}

const toasts = h("div", { class: "toasts", "aria-live": "polite" });

/** The browser's own yes/no and text questions, in the interface's language. */
const ask = (question: string) => confirm(tr(question));
const askText = (question: string) => prompt(tr(question));

function toast(message: string, isError = false) {
  const element = h("div", { class: isError ? "toast error" : "toast" }, message);
  toasts.append(element);
  setTimeout(() => element.remove(), isError ? 9000 : 5000);
}

/** Runs an action and reports its outcome, so no button fails silently. */
async function run(action: () => Promise<{ message?: string } | void>) {
  try {
    const result = await action();
    if (result?.message) toast(result.message);
  } catch (error) {
    toast((error as Error).message, true);
  }
  await refresh();
}

function socketUrl(path: string) {
  return `ws://${location.host}${path}?token=${token}`;
}

/** The sixteen terminal colors, tuned to stay readable on the pane's dark
 *  background: agents lean on them for diffs, warnings and prompts. */
const TERMINAL_THEME = {
  background: "#0b0d12",
  foreground: "#e8eaf0",
  cursor: "#a78bfa",
  cursorAccent: "#0b0d12",
  selectionBackground: "#3b3270",
  black: "#252936",
  red: "#f87171",
  green: "#34d399",
  yellow: "#fbbf24",
  blue: "#60a5fa",
  magenta: "#a78bfa",
  cyan: "#22d3ee",
  white: "#cfd3de",
  brightBlack: "#6b7285",
  brightRed: "#fca5a5",
  brightGreen: "#6ee7b7",
  brightYellow: "#fcd34d",
  brightBlue: "#93c5fd",
  brightMagenta: "#c4b5fd",
  brightCyan: "#67e8f9",
  brightWhite: "#ffffff",
};

function createPane(session: Session): Pane {
  const terminal = new Terminal({
    fontFamily: getComputedStyle(document.documentElement).getPropertyValue("--mono"),
    fontSize: 13,
    cursorBlink: true,
    scrollback: 8000,
    allowProposedApi: true,
    theme: TERMINAL_THEME,
  });
  const fit = new FitAddon();
  terminal.loadAddon(fit);

  const screen = h("div", { class: "pane-term" });
  const body = h("div", { class: "pane-body" }, screen);
  const stateLabel = h("span", { class: "state" });
  const filesLabel = h("span", { class: "state" });
  // Agents receive broadcast prompts by default; a plain terminal would run
  // them as commands.
  const target = h("input", {
    type: "checkbox",
    checked: session.kind !== "shell",
    title: "Reçoit les consignes envoyées à plusieurs agents",
    onchange: () => renderComposer(),
  });
  const element = h(
    "section",
    { class: "pane", "aria-label": session.title },
    h(
      "header",
      {
        class: "pane-head",
        draggable: "true",
        title: "Glisse pour changer l'ordre des panneaux",
        ondragstart: (event: DragEvent) => {
          dragging = session.title;
          event.dataTransfer?.setData("text/plain", session.title);
          if (event.dataTransfer) event.dataTransfer.effectAllowed = "move";
        },
        ondragend: () => {
          dragging = null;
          for (const pane of panes.values()) pane.element.classList.remove("drop");
        },
      },
      h("span", { class: `agent ${session.kind}` }, icon(session.kind)),
      h("span", { class: "title" }, session.title),
      h("span", { class: "status-dot" }),
      stateLabel,
      filesLabel,
      session.branch
        ? h(
            "span",
            {
              class: "own-branch",
              title: `Travaille dans son propre dossier, sur la branche ${session.branch}. Studio montre le projet tant que l'agent n'a pas connecté la synchro à sa branche.`,
            },
            icon("branch"),
            "branche à part",
          )
        : null,
      h("span", { class: "spacer" }),
      session.branch
        ? h(
            "button",
            {
              class: "merge",
              title: "Intègre le travail de cet agent au projet. Ce qu'il n'a pas commité l'est d'abord ; en cas de conflit, rien n'est modifié.",
              onclick: () => run(() => api(`/api/sessions/${session.id}/merge`, "POST")),
            },
            icon("merge"),
            "Fusionner",
          )
        : null,
      target,
      h(
        "button",
        {
          class: "quiet",
          title: "Agrandir ou réduire",
          onclick: () => {
            element.classList.toggle("zoomed");
            terminal.focus();
          },
        },
        icon("expand"),
      ),
      h("button", { class: "quiet", title: "Fermer la session", onclick: () => closeSession(session.id) }, icon("close")),
    ),
    body,
  );

  element.addEventListener("dragover", (event) => {
    if (!dragging || dragging === session.title) return;
    event.preventDefault();
    element.classList.add("drop");
  });
  element.addEventListener("dragleave", () => element.classList.remove("drop"));
  element.addEventListener("drop", (event) => {
    event.preventDefault();
    element.classList.remove("drop");
    if (dragging) movePane(session.project_id, dragging, session.title);
  });

  const socket = new WebSocket(socketUrl(`/ws/pty/${session.id}`));
  socket.binaryType = "arraybuffer";

  const send = (message: object) => {
    if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify(message));
  };

  // A session may have been running for a while, in another window or before
  // this one was reopened. What it printed was drawn for the size it had:
  // the terminal first takes that size and replays it, and only then goes to
  // the size of its pane and says so. Done in the other order, the agent's
  // interface is replayed at the wrong width and its lines pile up.
  let settled = false;
  const settle = () => {
    settled = true;
    applyFit();
    send({ t: "r", cols: terminal.cols, rows: terminal.rows });
  };
  let replaying = false;
  socket.onmessage = (event) => {
    if (typeof event.data === "string") {
      const start = JSON.parse(event.data) as { cols: number; rows: number; replay: number };
      terminal.resize(start.cols, start.rows);
      if (start.replay > 0) replaying = true;
      else settle();
      return;
    }
    const bytes = new Uint8Array(event.data as ArrayBuffer);
    if (replaying) {
      replaying = false;
      terminal.write(bytes, settle);
    } else {
      terminal.write(bytes);
    }
  };

  terminal.onData((data) => send({ t: "i", d: data }));
  terminal.onResize(({ cols, rows }) => {
    if (settled) send({ t: "r", cols, rows });
  });
  terminal.attachCustomKeyEventHandler((event) => {
    if (event.type !== "keydown") return true;
    if (event.ctrlKey && event.key === "c" && terminal.hasSelection()) {
      void navigator.clipboard.writeText(terminal.getSelection());
      terminal.clearSelection();
      return false;
    }
    // Let the browser turn Ctrl+V into a paste event, which xterm handles.
    if (event.ctrlKey && event.key === "v") return false;
    // Agents read Alt+Enter as "new line"; a bare Enter would submit.
    if (event.shiftKey && event.key === "Enter") {
      send({ t: "i", d: "\x1b\r" });
      return false;
    }
    return true;
  });

  // A hidden pane has no size, and a window being minimized or restored
  // passes through sizes of a few pixels: fitting to those would have the
  // agent redraw everything a few columns wide, for good as far as its
  // history goes.
  const applyFit = () => {
    if (document.hidden || body.clientWidth < 160 || body.clientHeight < 80) return;
    const proposed = fit.proposeDimensions();
    if (!proposed || proposed.cols < 20 || proposed.rows < 5) return;
    if (settled) fit.fit();
  };
  // Resizing fires many times a second; the agent redraws once it stops.
  let fitting = 0;
  const refit = () => {
    clearTimeout(fitting);
    fitting = window.setTimeout(applyFit, 120);
  };
  document.addEventListener("visibilitychange", refit);

  // xterm measures its character cell when it opens. Opened while detached it
  // measures nothing, every later fit is a no-op, and the terminal stays at
  // 80x24 inside a much larger pane.
  const mount = () => {
    terminal.open(screen);
    try {
      terminal.loadAddon(new WebglAddon());
    } catch {
      // The DOM renderer is slower but always available.
    }
    refit();
    new ResizeObserver(refit).observe(body);
    // The cell size changes once the terminal font has finished loading.
    void document.fonts.ready.then(refit);
  };

  return { mount, element, terminal, socket, state: stateLabel, files: filesLabel, target };
}

async function closeSession(id: string) {
  const session = state?.sessions.find((candidate) => candidate.id === id);
  if (session && !session.exited && !ask(`Arrêter « ${session.title} » ?`)) return;
  await run(() => api(`/api/sessions/${id}`, "DELETE"));
}

async function newSession(kind: Kind) {
  let created: string | undefined;
  await run(async () => {
    const session = await api<Session>("/api/sessions", "POST", {
      project_id: selected,
      kind,
      skip_permissions: skipPermissions,
      isolated,
      worktree: apart && kind !== "shell",
      model: kind === "claude" && model ? model : null,
    });
    created = session.id;
  });
  // The pane only exists once `run` has refreshed the state.
  if (created) panes.get(created)?.terminal.focus();
}

const dialog = h("dialog");

function openProjectDialog() {
  const name = h("input", { name: "name", required: true, autocomplete: "off" });
  const path = h("input", { name: "path", autocomplete: "off", placeholder: "Documents\\RoVibe\\<nom>" });
  const open = (state?.studios ?? []).filter((studio) => studio.context === "edit");
  const importStudio = h(
    "select",
    { disabled: open.length === 0 },
    h("option", { value: "" }, open.length === 0 ? "Aucune place ouverte dans Studio" : "Ne rien importer : projet vide"),
    ...open.map((studio) => h("option", { value: studio.id }, `Importer les scripts de « ${studio.name} »`)),
  );
  const form = h(
    "form",
    {
      method: "dialog",
      onsubmit: (event: Event) => {
        event.preventDefault();
        void run(async () => {
          const project = await api<Project & { message?: string }>("/api/projects", "POST", {
            name: name.value,
            path: path.value,
            import_studio: importStudio.value ? Number(importStudio.value) : null,
          });
          select(project.id);
          dialog.close();
          return project;
        });
      },
    },
    h("h2", {}, "Nouveau projet"),
    h("label", {}, h("span", {}, "Nom du jeu"), name),
    h(
      "label",
      {},
      h("span", {}, "Dossier (facultatif). Un dossier qui contient déjà un default.project.json est repris tel quel."),
      path,
    ),
    h(
      "label",
      {},
      h("span", {}, "Partir d'un jeu existant. Seuls les scripts deviennent des fichiers ; la place n'est pas modifiée."),
      importStudio,
    ),
    h(
      "div",
      { class: "actions" },
      h("button", { type: "button", onclick: () => dialog.close() }, "Annuler"),
      h("button", { class: "primary", type: "submit" }, "Créer le projet"),
    ),
  );
  dialog.replaceChildren(form);
  dialog.showModal();
}

interface History {
  enabled: boolean;
  dirty: number;
  commits: { hash: string; subject: string; when: string }[];
}

async function openHistoryDialog(project: Project) {
  const history = await api<History>(`/api/projects/${project.id}/git`);
  const act = (body: object) =>
    run(async () => {
      const result = await api(`/api/projects/${project.id}/git`, "POST", body);
      dialog.close();
      return result;
    });

  if (!history.enabled) {
    dialog.replaceChildren(
      h("h2", {}, "Historique"),
      h("p", { class: "notice" }, "Ce projet n'a pas son propre dépôt git : RoVibe ne gère pas son historique."),
      h("div", { class: "actions" }, h("button", { onclick: () => dialog.close() }, "Fermer")),
    );
    dialog.showModal();
    return;
  }

  const label = h("input", { placeholder: "Nom du point de sauvegarde (facultatif)", autocomplete: "off" });
  dialog.replaceChildren(
    h("h2", {}, "Historique"),
    h(
      "p",
      { class: "notice" },
      history.dirty === 0
        ? "Aucun changement depuis le dernier point de sauvegarde."
        : `${history.dirty} fichier(s) modifié(s) depuis le dernier point de sauvegarde.`,
    ),
    h(
      "form",
      {
        class: "snapshot",
        onsubmit: (event: Event) => {
          event.preventDefault();
          void act({ action: "snapshot", message: label.value });
        },
      },
      label,
      h("button", { class: "primary", type: "submit", disabled: history.dirty === 0 }, "Créer un point de sauvegarde"),
    ),
    h(
      "ul",
      { class: "bank" },
      ...history.commits.map((commit, index) =>
        h(
          "li",
          {},
          h("div", {}, h("strong", {}, commit.subject), h("small", {}, `${commit.when}, ${commit.hash.slice(0, 7)}`)),
          index === 0 && history.dirty === 0
            ? h("small", {}, "État actuel")
            : h(
                "button",
                {
                  class: "quiet",
                  title: "Remet les fichiers dans cet état. L'état actuel est d'abord sauvegardé, rien n'est perdu.",
                  onclick: () => {
                    if (ask(`Revenir à « ${commit.subject} » ? L'état actuel sera sauvegardé avant.`)) {
                      void act({ action: "restore", commit: commit.hash });
                    }
                  },
                },
                "Revenir ici",
              ),
        ),
      ),
    ),
    h("div", { class: "actions" }, h("button", { onclick: () => dialog.close() }, "Fermer")),
  );
  dialog.showModal();
}

interface BankAsset {
  id: string;
  name: string;
  tags: string[];
  class: string;
  instances: number;
  bytes: number;
  collection: string;
  thumb: boolean;
}

interface StoreItem {
  id: number;
  name: string;
  creator: string;
  verified: boolean;
  votes: [number, number] | null;
  triangles: number | null;
  has_scripts: boolean;
  thumbnail: string | null;
}

interface Change {
  path: string;
  status: "added" | "modified" | "deleted";
  added: number;
  deleted: number;
  agents: string[];
}

const CHANGE_LABELS = { added: "nouveau", modified: "modifié", deleted: "supprimé" };

function widen() {
  dialog.classList.add("wide");
  dialog.addEventListener("close", () => dialog.classList.remove("wide"), { once: true });
}

async function openChangesDialog(project: Project) {
  const { changes } = await api<{ changes: Change[] }>(`/api/projects/${project.id}/changes`);
  const view = h("pre", { class: "log diff", tabindex: 0 });
  const list = h("ul", { class: "bank changes" });
  let shown: string | null = null;

  const show = async (path: string) => {
    shown = path;
    for (const item of list.children) item.toggleAttribute("aria-current", (item as HTMLElement).dataset.path === path);
    try {
      const { diff } = await api<{ diff: string }>(`/api/projects/${project.id}/changes/diff?path=${encodeURIComponent(path)}`);
      // Lines are colored by what they do to the file, the way a diff reads.
      view.replaceChildren(
        ...diff.split("\n").map((line) => {
          const kind = line.startsWith("+++") || line.startsWith("---") || line.startsWith("diff ") || line.startsWith("index ")
            ? "meta"
            : line.startsWith("@@")
              ? "hunk"
              : line.startsWith("+")
                ? "plus"
                : line.startsWith("-")
                  ? "minus"
                  : "";
          return h("span", kind ? { class: kind } : {}, line + "\n");
        }),
      );
      view.scrollTop = 0;
    } catch (error) {
      view.textContent = (error as Error).message;
    }
  };

  const review = (body: object) =>
    run(async () => {
      const result = await api(`/api/projects/${project.id}/changes`, "POST", body);
      dialog.close();
      return result;
    });

  list.replaceChildren(
    ...changes.map((change) =>
      h(
        "li",
        { "data-path": change.path },
        h(
          "button",
          { class: "quiet file", onclick: () => show(change.path) },
          h("strong", {}, change.path),
          h(
            "small",
            {},
            [
              CHANGE_LABELS[change.status],
              `+${change.added} −${change.deleted}`,
              change.agents.length ? change.agents.join(", ") : "auteur inconnu (session fermée ou modification à la main)",
            ].join(", "),
          ),
        ),
        h(
          "button",
          {
            class: "quiet",
            title: "Remet ce fichier dans l'état de ta dernière relecture",
            onclick: () => {
              const what = change.status === "added" ? "Supprimer ce nouveau fichier" : "Annuler les changements de";
              if (ask(`${what} ${change.path} ?`)) void review({ action: "revert", path: change.path });
            },
          },
          "Annuler",
        ),
      ),
    ),
  );

  dialog.replaceChildren(
    h("h2", {}, "Changements"),
    h(
      "p",
      { class: "notice" },
      changes.length === 0
        ? "Rien n'a changé depuis ta dernière relecture."
        : `${changes.length} fichier(s) modifié(s) depuis ta dernière relecture, commités ou non par les agents.`,
    ),
    changes.length > 0 ? h("div", { class: "review" }, list, view) : "",
    h(
      "div",
      { class: "actions" },
      h("button", { onclick: () => dialog.close() }, "Fermer"),
      h(
        "button",
        {
          class: "primary",
          disabled: changes.length === 0,
          title: "Crée un point de sauvegarde et repart de cet état pour la prochaine relecture",
          onclick: () => review({ action: "accept" }),
        },
        "Tout accepter",
      ),
    ),
  );
  widen();
  dialog.showModal();
  if (changes[0]) void show(changes[0].path);
  void shown;
}

interface Settings {
  claude_model: string;
  codex_model: string;
  projects_dir: string;
  publish_shortcut: string;
  isolation_network: string;
  isolation_hosts: string;
  notifications: string;
}

/** What the main area shows: the selected project, or the settings. */
let view: "project" | "settings" = sessionStorage.getItem("rovibe.view") === "settings" ? "settings" : "project";
const settingsPage = h("section", { class: "settings", hidden: true });
/** Brings the page's status lines up to date; its fields are left alone, so
 *  that a refresh never interrupts typing. */
let settingsStatus: ((current: State) => void) | null = null;

function closeSettings() {
  view = "project";
  sessionStorage.removeItem("rovibe.view");
  render();
}

async function openSettings() {
  view = "settings";
  sessionStorage.setItem("rovibe.view", "settings");
  const settings = await api<Settings>("/api/settings");
  const values: Settings = { ...settings };

  // Every change is saved as it is made; this says so, briefly.
  const saved = h("span", { class: "saved", role: "status" });
  let fading = 0;
  const confirmSaved = () => {
    saved.textContent = tr("Enregistré");
    saved.classList.add("on");
    clearTimeout(fading);
    fading = window.setTimeout(() => saved.classList.remove("on"), 1800);
  };
  // A value the server refuses never enters `values`: it would make every
  // later save fail with it. The field goes back to what is really stored.
  const save = async (key: keyof Settings, value: string, restore: (stored: string) => void) => {
    try {
      await api("/api/settings", "PUT", { ...values, [key]: value });
      values[key] = value;
      confirmSaved();
    } catch (error) {
      toast((error as Error).message, true);
      restore(values[key]);
    }
  };

  const text = (key: keyof Settings, placeholder: string) => {
    const input = h("input", { value: settings[key], placeholder, autocomplete: "off", spellcheck: "false" });
    input.addEventListener("change", () => void save(key, input.value.trim(), (stored) => (input.value = stored)));
    return input;
  };
  const choice = (current: string, options: [string, string][], onChange: (value: string, select: HTMLSelectElement) => void) => {
    const select = h("select", {}, ...options.map(([value, label]) => h("option", { value }, label)));
    select.value = options.some(([value]) => value === current) ? current : options[0][0];
    select.addEventListener("change", () => onChange(select.value, select));
    return select;
  };
  const serverChoice = (key: keyof Settings, options: [string, string][]) =>
    choice(settings[key], options, (value, select) => void save(key, value, (stored) => (select.value = stored)));
  const local = (key: string, options: [string, string][], after?: () => void) =>
    choice(localStorage.getItem(key) ?? "", options, (value) => {
      if (value) localStorage.setItem(key, value);
      else localStorage.removeItem(key);
      confirmSaved();
      after?.();
    });

  const row = (label: string, hint: string, control: Child) =>
    h("div", { class: "setting" }, h("div", { class: "about" }, h("strong", {}, label), hint ? h("small", {}, hint) : null), h("div", { class: "control" }, control));
  const state_ = (name: string) => {
    const dot = h("span", { class: "dot" });
    const label = h("span", {});
    return { element: h("span", { class: "state-line", "data-name": name }, dot, label), dot, label };
  };
  const sections: [string, string, string, Child[]][] = [];
  const section = (id: string, title: string, intro: string, ...rows: Child[]) => sections.push([id, title, intro, rows]);

  const claude = state_("claude");
  const codex = state_("codex");
  const plugin = state_("plugin");
  const studio = state_("studio");
  const distro = state_("distro");
  const version = h("strong", {});
  const update = h("div", { class: "control" });
  const pluginButton = h("button", { onclick: () => run(() => api("/api/plugin/install", "POST")) });

  const hosts = h("textarea", { rows: 3, placeholder: "ex. github.com *.githubusercontent.com", spellcheck: "false" });
  hosts.value = settings.isolation_hosts;
  hosts.addEventListener("change", () => void save("isolation_hosts", hosts.value.trim(), (stored) => (hosts.value = stored)));

  section(
    "general",
    "Général",
    "Ce qui tient à l'app elle-même.",
    row(
      "Langue",
      "L'interface suit la langue du système, ou celle que tu choisis ici.",
      local(
        "rovibe.lang",
        [
          ["", "Celle du système"],
          ["fr", "Français"],
          ["en", "English"],
        ],
        // The page is built in one language: a change starts it again.
        () => location.reload(),
      ),
    ),
    row(
      "Fermer la fenêtre pendant que des sessions tournent",
      "En arrière-plan, les agents continuent et l'icône près de l'horloge rouvre la fenêtre.",
      local("rovibe.close", [
        ["", "Me demander"],
        ["hide", "Continuer en arrière-plan"],
        ["quit", "Arrêter les agents et quitter"],
      ]),
    ),
    row(
      "Notifications Windows",
      "Elles ne partent que si la fenêtre n'est pas devant.",
      serverChoice("notifications", [
        ["", "Quand un agent m'attend ou a fini"],
        ["waiting", "Seulement quand un agent m'attend"],
        ["off", "Jamais"],
      ]),
    ),
    row(
      "Dossier des nouveaux projets",
      "Chemin complet. Vide : Documents\\RoVibe.",
      text("projects_dir", "Documents\\RoVibe"),
    ),
  );
  section(
    "agents",
    "Agents",
    "Les modèles donnés aux nouvelles sessions. Celui de Claude Code se choisit aussi au lancement.",
    row("Claude Code", "", claude.element),
    row("Modèle de Claude Code", "Vide : celui que Claude Code choisit lui-même.", text("claude_model", "ex. opus, sonnet, haiku")),
    row("Codex", "", codex.element),
    row("Modèle de Codex", "Vide : celui que Codex choisit lui-même.", text("codex_model", "")),
  );
  section(
    "studio",
    "Roblox Studio",
    "Le lien entre l'app et Studio.",
    row("Plugin RoVibe Studio", "Après une installation ou une mise à jour, redémarre Studio pour le charger.", h("div", { class: "control-stack" }, plugin.element, pluginButton)),
    row("Studio", "", studio.element),
    row(
      "Raccourci « Publier sur Roblox »",
      "À changer seulement si tu l'as changé dans Studio. Touches séparées par +.",
      text("publish_shortcut", "alt+p"),
    ),
  );
  section(
    "isolation",
    "Agents isolés",
    "Un agent lancé avec « Isolé » tourne dans une distribution WSL où il ne voit que son projet.",
    row("Distribution WSL", "Créée une fois par scripts\\setup-isolation.ps1.", distro.element),
    row(
      "Réseau",
      "Restreint, un agent isolé ne joint que l'API de son modèle et les hôtes ci-dessous.",
      serverChoice("isolation_network", [
        ["", "Restreint"],
        ["open", "Ouvert : tout internet"],
      ]),
    ),
    row("Hôtes autorisés en plus", "En HTTPS. Un nom par hôte, ou *.domaine pour tout un domaine.", hosts),
  );
  section(
    "about",
    "À propos",
    "",
    row("Version", "", version),
    row("Mise à jour", "L'app en cherche une à chaque démarrage.", update),
    row(
      "Diagnostic",
      "Ce que l'app a fait, et ce dont elle a besoin sur ce PC.",
      h(
        "div",
        { class: "control-stack" },
        h("button", { onclick: () => run(openLogDialog) }, icon("log"), "Journal"),
        h("button", { onclick: openSetupDialog }, icon("setup"), "Premiers pas"),
      ),
    ),
    row("Code source", "", h("code", {}, "github.com/aaldersondev/rovibe")),
  );

  const content = h("div", { class: "settings-content" });
  settingsPage.replaceChildren(
    h(
      "header",
      { class: "settings-head" },
      h("button", { class: "quiet", title: "Revenir au projet", onclick: closeSettings }, icon("back"), "Retour"),
      h("h1", {}, "Réglages"),
      saved,
    ),
    h(
      "div",
      { class: "settings-body" },
      h(
        "nav",
        { class: "settings-nav", "aria-label": "Sections" },
        ...sections.map(([id, title]) =>
          h("button", { class: "quiet", onclick: () => document.getElementById(`settings-${id}`)?.scrollIntoView({ behavior: "smooth", block: "start" }) }, title),
        ),
      ),
      content,
    ),
  );
  content.replaceChildren(
    ...sections.map(([id, title, intro, rows]) =>
      h("section", { class: "settings-section", id: `settings-${id}` }, h("h2", {}, title), intro ? h("p", { class: "notice" }, intro) : null, h("div", { class: "settings-card" }, ...rows)),
    ),
  );

  const show = (line: ReturnType<typeof state_>, ok: boolean, yes: string, no: string) => {
    line.dot.className = ok ? "dot on" : "dot off";
    line.label.textContent = tr(ok ? yes : no);
  };
  settingsStatus = (current) => {
    show(claude, current.tools.claude, "Installé", "Introuvable dans le PATH");
    show(codex, current.tools.codex, "Installé", "Introuvable dans le PATH");
    show(plugin, current.plugin_installed, "Installé", "Pas installé");
    pluginButton.textContent = tr(current.plugin_installed ? "Réinstaller" : "Installer le plugin");
    pluginButton.hidden = !current.tools.sync;
    const editors = current.studios.filter((studio) => studio.context === "edit");
    show(studio, editors.length > 0, editors.map((studio) => studio.name).join(", ") || "Connecté", "Non connecté");
    show(distro, current.isolation, "Installée", "Pas installée");
    version.textContent = `RoVibe ${current.version}`;
    update.replaceChildren(
      current.update
        ? h("button", { class: "primary", onclick: () => run(() => api("/api/update", "POST")) }, `Installer RoVibe ${current.update}`)
        : h("span", { class: "state-line" }, h("span", { class: "dot on" }), "Aucune mise à jour en attente"),
    );
  };
  render();
}

async function openLogDialog() {
  const log = await api<{ path: string | null; lines: string[] }>("/api/log");
  const text = h("pre", { class: "log", tabindex: 0 }, log.lines.join("\n") || "Le journal est vide.");
  dialog.replaceChildren(
    h("h2", {}, "Journal"),
    h("p", { class: "notice" }, "Ce que l'app a fait, du plus ancien au plus récent : sessions, synchro, connexions de Studio, outils en erreur, mises à jour."),
    text,
    h("p", { class: "notice" }, `Fichier : ${log.path ?? "indisponible"}`),
    h("div", { class: "actions" }, h("button", { onclick: () => dialog.close() }, "Fermer")),
  );
  widen();
  dialog.showModal();
  text.scrollTop = text.scrollHeight;
}

async function openBankDialog() {
  const bank = await api<{ dir: string; assets: BankAsset[] }>("/api/assets");
  const reload = async () => {
    bank.assets = (await api<{ assets: BankAsset[] }>("/api/assets")).assets;
    draw();
  };

  // What the bank holds.
  const filter = h("input", { type: "search", placeholder: "Filtrer par nom, tag ou collection", autocomplete: "off" });
  const collections = h("select", { title: "Collection" });
  const names = h("datalist", { id: "bank-collections" });
  const list = h("ul", { class: "bank" });
  let collection = "";

  const draw = () => {
    const known = [...new Set(bank.assets.map((asset) => asset.collection).filter(Boolean))].sort((a, b) =>
      a.localeCompare(b),
    );
    if (collection && !known.includes(collection)) collection = "";
    collections.replaceChildren(
      h("option", { value: "" }, `Toutes les collections (${bank.assets.length})`),
      ...known.map((name) =>
        h("option", { value: name }, `${name} (${bank.assets.filter((asset) => asset.collection === name).length})`),
      ),
    );
    collections.value = collection;
    names.replaceChildren(...known.map((name) => h("option", { value: name })));

    const words = filter.value.toLowerCase().split(/\s+/).filter(Boolean);
    const shown = bank.assets.filter((asset) => {
      if (collection && asset.collection !== collection) return false;
      const haystack = `${asset.name} ${asset.tags.join(" ")} ${asset.class} ${asset.collection}`.toLowerCase();
      return words.every((word) => haystack.includes(word));
    });

    list.replaceChildren(
      ...shown.map((asset) => {
        const place = h("input", {
          class: "collection",
          value: asset.collection,
          placeholder: "Sans collection",
          title: "Collection de cet asset",
          autocomplete: "off",
        });
        place.setAttribute("list", "bank-collections");
        place.addEventListener("change", async () => {
          try {
            await api(`/api/assets/${asset.id}`, "PUT", { collection: place.value });
            asset.collection = place.value.trim();
            draw();
          } catch (error) {
            toast((error as Error).message, true);
          }
        });

        return h(
          "li",
          {},
          asset.thumb
            ? h("img", { class: "thumb", src: `/api/assets/${asset.id}/thumb?token=${token}`, alt: "", loading: "lazy" })
            : h("span", { class: "thumb" }),
          h(
            "div",
            { class: "about" },
            h("strong", {}, asset.name),
            h(
              "small",
              {},
              [
                asset.class || tr("fichier ajouté à la main"),
                asset.instances ? `${asset.instances} instances` : "",
                `${Math.max(1, Math.round(asset.bytes / 1024))} Ko`,
                asset.tags.join(", "),
              ]
                .filter(Boolean)
                .join(", "),
            ),
            h("code", {}, `bank:${asset.id}`),
          ),
          place,
          h(
            "button",
            {
              class: "quiet",
              title: "Supprimer de la banque",
              onclick: async () => {
                if (!ask(`Supprimer « ${asset.name} » de la banque ?`)) return;
                try {
                  await api(`/api/assets/${asset.id}`, "DELETE");
                  bank.assets = bank.assets.filter((other) => other.id !== asset.id);
                  draw();
                } catch (error) {
                  toast((error as Error).message, true);
                }
              },
            },
            "Supprimer",
          ),
        );
      }),
    );
    if (shown.length === 0) {
      list.append(
        h(
          "li",
          { class: "notice" },
          bank.assets.length === 0
            ? "La banque est vide. Demande à un agent « enregistre Workspace.MonModele dans la banque », importe un pack, ou dépose des fichiers .rbxm dans le dossier ci-dessous."
            : "Aucun asset ne correspond.",
        ),
      );
    }
    missing.hidden = !bank.assets.some((asset) => !asset.thumb);
  };

  filter.addEventListener("input", draw);
  collections.addEventListener("change", () => {
    collection = collections.value;
    draw();
  });

  const missing = h(
    "button",
    {
      title: "Chaque asset sans image est posé un instant dans la place ouverte dans Studio, photographié, puis retiré",
      onclick: async () => {
        missing.disabled = true;
        missing.textContent = tr("Aperçus en cours…");
        try {
          const done = await api<{ made: number; failed: string[] }>("/api/assets/previews", "POST");
          toast(`${done.made} aperçu(s) créé(s)${done.failed.length ? `, ${done.failed.length} impossible(s)` : ""}`);
          for (const reason of done.failed.slice(0, 3)) toast(reason, true);
          await reload();
        } catch (error) {
          toast((error as Error).message, true);
        }
        missing.disabled = false;
        missing.textContent = tr("Créer les aperçus manquants");
      },
    },
    "Créer les aperçus manquants",
  );

  const packPath = h("input", { placeholder: "Dossier du pack, ex. C:\\Packs\\Nature", autocomplete: "off" });
  const packName = h("input", { placeholder: "Collection (facultatif)", autocomplete: "off" });
  packName.setAttribute("list", "bank-collections");
  const importer = h(
    "form",
    {
      class: "pack",
      onsubmit: async (event: Event) => {
        event.preventDefault();
        if (!packPath.value.trim()) return;
        try {
          const done = await api<{ imported: number }>("/api/assets/import", "POST", {
            path: packPath.value,
            collection: packName.value,
          });
          toast(`${done.imported} asset(s) importé(s)`);
          packPath.value = "";
          await reload();
        } catch (error) {
          toast((error as Error).message, true);
        }
      },
    },
    packPath,
    packName,
    h("button", { type: "submit" }, "Importer le pack"),
  );

  const local = h(
    "div",
    {},
    h("div", { class: "pack" }, filter, collections, missing),
    list,
    names,
    importer,
    h(
      "p",
      { class: "notice" },
      `Un pack est un dossier de fichiers .rbxm ou .rbxmx ; ses sous-dossiers deviennent des collections. Dossier de la banque : ${bank.dir}`,
    ),
  );

  // The Creator Store, with pictures.
  const query = h("input", { type: "search", placeholder: "Chercher dans le Creator Store (gratuits)", autocomplete: "off" });
  const kind = h(
    "select",
    {},
    h("option", { value: "model" }, "Modèles"),
    h("option", { value: "mesh" }, "Meshes"),
    h("option", { value: "decal" }, "Images"),
    h("option", { value: "audio" }, "Sons"),
  );
  const results = h("ul", { class: "store" }, h("li", { class: "notice" }, "Tape une recherche."));

  const searchStore = async (event: Event) => {
    event.preventDefault();
    if (!query.value.trim()) return;
    results.replaceChildren(h("li", { class: "notice" }, "Recherche…"));
    try {
      const searched = kind.value;
      const found = await api<{ items: StoreItem[] }>(
        `/api/store?q=${encodeURIComponent(query.value)}&kind=${searched}`,
      );
      results.replaceChildren(
        ...found.items.map((item) =>
          h(
            "li",
            {},
            item.thumbnail
              ? h("img", { src: item.thumbnail, alt: "", loading: "lazy", referrerpolicy: "no-referrer" })
              : h("span", { class: "blank" }),
            h("strong", { title: item.name }, item.name),
            h(
              "small",
              {},
              [
                `${item.creator}${item.verified ? " ✓" : ""}`,
                item.votes ? `${item.votes[0]} % / ${item.votes[1]}` : "",
                item.triangles ? `${item.triangles} triangles` : "",
              ]
                .filter(Boolean)
                .join(", "),
            ),
            searched === "model"
              ? h("small", { class: item.has_scripts ? "warn" : "" }, item.has_scripts ? "Contient des scripts" : "Sans script")
              : null,
            h(
              "button",
              {
                title: "Pose l'asset dans Workspace, scripts désactivés",
                onclick: () =>
                  run(async () => {
                    const done = await api("/api/store/insert", "POST", {
                      project_id: selected,
                      asset: item.id,
                      kind: searched,
                    });
                    return { message: (done.message ?? "Inséré").split("\n").slice(0, 2).join(" ") };
                  }),
              },
              "Insérer dans Studio",
            ),
          ),
        ),
      );
      if (found.items.length === 0) results.append(h("li", { class: "notice" }, "Aucun asset gratuit ne correspond."));
    } catch (error) {
      results.replaceChildren(h("li", { class: "notice" }, (error as Error).message));
    }
  };

  const store = h(
    "div",
    { hidden: true },
    h("form", { class: "pack", onsubmit: searchStore }, query, kind, h("button", { type: "submit" }, "Chercher")),
    results,
    h(
      "p",
      { class: "notice" },
      "Les scripts d'un asset du Store sont désactivés à l'insertion : un modèle gratuit peut cacher une porte dérobée.",
    ),
  );

  const tabs = [
    { label: "Ma banque", view: local },
    { label: "Creator Store", view: store },
  ].map(({ label, view }) => {
    const tab = h(
      "button",
      {
        class: "quiet",
        onclick: () => {
          local.hidden = view !== local;
          store.hidden = view !== store;
          for (const other of tabs) other.removeAttribute("aria-current");
          tab.setAttribute("aria-current", "true");
        },
      },
      label,
    );
    return tab;
  });
  tabs[0].setAttribute("aria-current", "true");

  draw();
  dialog.replaceChildren(
    h("h2", {}, "Banque d'assets"),
    h(
      "p",
      { class: "notice" },
      "Modèles réutilisables d'un projet à l'autre. Les agents les trouvent avec asset_search, les regardent avec asset_preview et les posent avec asset_insert, tout comme les assets gratuits du Creator Store.",
    ),
    h("div", { class: "tabs" }, ...tabs),
    local,
    store,
    h("div", { class: "actions" }, h("button", { onclick: () => dialog.close() }, "Fermer")),
  );
  widen();
  dialog.showModal();
}

function select(id: string) {
  view = "project";
  sessionStorage.removeItem("rovibe.view");
  selected = id;
  localStorage.setItem("rovibe.project", id);
  render();
}

const STATUS_LABELS: Record<string, string> = {
  starting: "démarre",
  working: "travaille",
  waiting: "attend ta réponse",
  idle: "a fini",
};

function describeStatus(session: Session) {
  const label = STATUS_LABELS[session.status];
  if (!label) return "";
  // The detail of a finished turn is stale; the others say what is going on.
  // Each part is translated on its own: the detail comes from the server.
  return session.detail && session.status !== "idle" ? `${tr(label)} : ${tr(session.detail)}` : tr(label);
}

/** Statuses seen at the last refresh, to tell a change from a steady state. */
const lastStatus = new Map<string, string>();
let unseen = 0;

function announceChanges(current: State) {
  for (const session of current.sessions) {
    const before = lastStatus.get(session.id);
    lastStatus.set(session.id, session.status);
    if (before !== "working" || session.exited) continue;

    if (session.status === "waiting" || session.status === "idle") {
      toast(`${session.title} ${tr(STATUS_LABELS[session.status])}`);
      if (!document.hasFocus()) unseen += 1;
    }
  }
  document.title = unseen > 0 ? `(${unseen}) RoVibe` : "RoVibe";
}

window.addEventListener("focus", () => {
  unseen = 0;
  document.title = "RoVibe";
});

interface Prompt {
  name: string;
  text: string;
}

const prompts = new Map<string, Prompt[]>();
const composer = h("form", { class: "composer" });
const draft = h("textarea", {
  rows: 1,
  "aria-label": "Consigne",
  placeholder: "Écris une consigne pour les agents sélectionnés…",
});

/** The field is one line tall and grows with what is typed, up to a point. */
function fitDraft() {
  draft.style.height = "auto";
  draft.style.height = `${Math.min(draft.scrollHeight + 2, 168)}px`;
}
draft.addEventListener("keydown", (event) => {
  if (event.key === "Enter" && !event.shiftKey) {
    event.preventDefault();
    broadcast();
  }
});
draft.addEventListener("input", () => renderComposer());
window.addEventListener("resize", fitDraft);

function targets() {
  return (state?.sessions ?? []).filter(
    (session) => session.project_id === selected && !session.exited && panes.get(session.id)?.target.checked,
  );
}

function broadcast() {
  const text = draft.value.trim();
  const chosen = targets();
  if (!text || chosen.length === 0) return;

  for (const session of chosen) {
    const socket = panes.get(session.id)?.socket;
    if (socket?.readyState !== WebSocket.OPEN) continue;
    // Bracketed paste keeps the line breaks inside the prompt; a bare
    // newline would submit it half-written. Enter follows once the agent
    // has taken the paste in.
    socket.send(JSON.stringify({ t: "i", d: `\x1b[200~${text}\x1b[201~` }));
    setTimeout(() => socket.send(JSON.stringify({ t: "i", d: "\r" })), 250);
  }
  toast(chosen.length === 1 ? `Consigne envoyée à ${chosen[0].title}` : `Consigne envoyée à ${chosen.length} agents`);
  draft.value = "";
  renderComposer();
}

async function loadPrompts(projectId: string) {
  if (!prompts.has(projectId)) {
    prompts.set(projectId, []);
    try {
      prompts.set(projectId, await api<Prompt[]>(`/api/projects/${projectId}/prompts`));
    } catch {
      // A project without saved prompts simply has an empty list.
    }
    renderComposer();
  }
}

async function savePrompts(projectId: string, list: Prompt[]) {
  prompts.set(projectId, list);
  renderComposer();
  try {
    await api(`/api/projects/${projectId}/prompts`, "PUT", list);
  } catch (error) {
    toast((error as Error).message, true);
  }
}

function renderComposer() {
  const projectId = selected;
  const visible = (state?.sessions ?? []).some((session) => session.project_id === projectId && !session.exited);
  composer.hidden = !projectId || !visible;
  if (!projectId || !visible) return;
  void loadPrompts(projectId);

  const saved = prompts.get(projectId) ?? [];
  const count = targets().length;
  const picker = h(
    "select",
    {
      title: "Consignes enregistrées dans ce projet",
      onchange: (event: Event) => {
        const chosen = saved[Number((event.target as HTMLSelectElement).value)];
        if (chosen) draft.value = chosen.text;
        renderComposer();
        draft.focus();
      },
    },
    h("option", { value: "" }, saved.length === 0 ? "Aucune consigne enregistrée" : "Consignes enregistrées"),
    ...saved.map((prompt, index) => h("option", { value: index }, prompt.name)),
  );
  const current = saved.findIndex((prompt) => prompt.text === draft.value.trim());

  const live = ordered(state?.sessions ?? [], projectId).filter((session) => !session.exited);
  const chosen = new Set(targets().map((session) => session.id));
  const everyone = live.length > 0 && chosen.size === live.length;
  const choose = (ids: string[], on: boolean) => {
    for (const id of ids) {
      const pane = panes.get(id);
      if (pane) pane.target.checked = on;
    }
    renderComposer();
  };

  composer.replaceChildren(
    h(
      "div",
      { class: "composer-to" },
      h("span", { class: "label" }, "Pour"),
      h(
        "button",
        {
          type: "button",
          class: "chip",
          "aria-pressed": String(everyone),
          title: everyone ? "Ne plus viser personne" : "Viser toutes les sessions",
          onclick: () => choose(live.map((session) => session.id), !everyone),
        },
        "Tous",
      ),
      ...live.map((session) =>
        h(
          "button",
          {
            type: "button",
            class: "chip",
            "aria-pressed": String(chosen.has(session.id)),
            "data-status": session.status,
            title: describeStatus(session),
            onclick: () => choose([session.id], !chosen.has(session.id)),
          },
          h("span", { class: `agent ${session.kind}` }, icon(session.kind)),
          session.title,
        ),
      ),
      h("span", { class: "spacer" }),
      h("span", { class: "keys" }, h("kbd", {}, "Entrée"), "envoyer", h("kbd", {}, "Maj"), "+", h("kbd", {}, "Entrée"), "nouvelle ligne"),
    ),
    h(
      "div",
      { class: "composer-row" },
      h(
        "div",
        { class: "composer-saved" },
        picker,
        current >= 0
          ? h(
              "button",
              {
                type: "button",
                title: "Supprimer cette consigne enregistrée",
                onclick: () => {
                  if (ask(`Supprimer la consigne « ${saved[current].name} » ?`)) {
                    void savePrompts(projectId, saved.filter((_, index) => index !== current));
                  }
                },
              },
              icon("close"),
              "Supprimer",
            )
          : h(
              "button",
              {
                type: "button",
                title: "Garder cette consigne dans le projet pour la réutiliser",
                disabled: !draft.value.trim(),
                onclick: () => {
                  const name = askText("Nom de la consigne");
                  if (name?.trim()) void savePrompts(projectId, [...saved, { name: name.trim(), text: draft.value.trim() }]);
                },
              },
              icon("save"),
              "Enregistrer",
            ),
      ),
      draft,
      h(
        "button",
        { class: "primary", type: "submit", disabled: count === 0 || !draft.value.trim() },
        icon("send"),
        count === 0 ? "Aucun destinataire" : count === 1 ? "Envoyer à 1 agent" : `Envoyer aux ${count} agents`,
      ),
    ),
  );
  fitDraft();
}

composer.addEventListener("submit", (event) => {
  event.preventDefault();
  broadcast();
});

/** What RoVibe needs on this PC, what it has, and how to get the rest. */
function setupItems(current: State) {
  return [
    {
      name: "Claude Code",
      ok: current.tools.claude,
      needed: !current.tools.codex,
      hint: "Installe-le avec « npm install -g @anthropic-ai/claude-code », connecte-toi une fois en lançant « claude » dans un terminal, puis relance RoVibe.",
    },
    {
      name: "Codex",
      ok: current.tools.codex,
      needed: false,
      hint: "Facultatif. Installe-le avec « npm install -g @openai/codex », puis relance RoVibe.",
    },
    {
      name: "Git",
      ok: current.tools.git,
      needed: true,
      hint: "Sert à l'historique, à la relecture des changements et aux branches à part. À installer depuis git-scm.com.",
    },
    {
      name: "Moteur de synchro",
      ok: current.tools.sync,
      needed: true,
      hint: "rovibe-sync.exe manque à côté de l'app : réinstalle RoVibe, ou compile vendor/sync.",
    },
    {
      name: "Plugin Roblox Studio",
      ok: current.plugin_installed,
      needed: true,
      hint: "Le plugin relie Studio à RoVibe. Installe-le, puis redémarre Studio.",
      action: current.tools.sync
        ? { label: "Installer le plugin", run: () => run(() => api("/api/plugin/install", "POST")) }
        : undefined,
    },
    {
      name: "Roblox Studio connecté",
      ok: current.studios.length > 0,
      needed: false,
      hint: "Ouvre une place dans Studio. S'il était ouvert pendant l'installation du plugin, redémarre-le.",
    },
    {
      name: "Vérification du code",
      ok: current.checkers.selene && current.checkers.luau_lsp,
      needed: false,
      hint: "Facultatif : selene et luau-lsp relisent le code des agents. Lance scripts\\get-tools.ps1 pour les installer.",
    },
    {
      name: "Agents isolés",
      ok: current.isolation,
      needed: false,
      hint: "Facultatif : une distribution WSL où un agent ne voit que son projet. Lance scripts\\setup-isolation.ps1 une fois.",
    },
  ];
}

let setupOpen = false;

function drawSetup() {
  if (!state) return;
  const items = setupItems(state);
  const missing = items.filter((item) => item.needed && !item.ok).length;
  dialog.replaceChildren(
    h("h2", {}, "Premiers pas"),
    h(
      "p",
      { class: "notice" },
      missing === 0
        ? "Tout ce qu'il faut est en place. Le reste est facultatif."
        : "Voici ce qu'il reste à mettre en place pour que les agents puissent travailler sur ton jeu.",
    ),
    h(
      "ul",
      { class: "setup" },
      ...items.map((item) =>
        h(
          "li",
          { class: item.ok ? "ok" : item.needed ? "todo" : "optional" },
          icon(item.ok ? "check" : "alert"),
          h("div", {}, h("strong", {}, item.name), item.ok ? null : h("small", {}, item.hint)),
          !item.ok && item.action ? h("button", { onclick: item.action.run }, item.action.label) : null,
        ),
      ),
    ),
    h(
      "div",
      { class: "actions" },
      h("button", { onclick: () => dialog.close() }, "Fermer"),
      state.projects.length === 0
        ? h(
            "button",
            {
              class: "primary",
              onclick: () => {
                dialog.close();
                openProjectDialog();
              },
            },
            "Créer mon premier projet",
          )
        : null,
    ),
  );
}

function openSetupDialog() {
  setupOpen = true;
  dialog.addEventListener("close", () => (setupOpen = false), { once: true });
  drawSetup();
  if (!dialog.open) dialog.showModal();
}

/** Called by the desktop window when the user closes it while sessions run:
 *  closing would end them, so the page asks what was meant. */
function askBeforeClosing() {
  const answer = (action: "hide" | "quit") => {
    dialog.close();
    void api("/api/window", "POST", { action }).catch((error) => toast((error as Error).message, true));
  };
  const remembered = localStorage.getItem("rovibe.close");
  if (remembered === "hide" || remembered === "quit") {
    answer(remembered);
    return;
  }

  const live = (state?.sessions ?? []).filter((session) => !session.exited).length;
  const remember = h("input", { type: "checkbox" });
  const choose = (action: "hide" | "quit") => {
    if (remember.checked) localStorage.setItem("rovibe.close", action);
    answer(action);
  };
  dialog.replaceChildren(
    h("h2", {}, live === 1 ? "1 session est en cours" : `${live} sessions sont en cours`),
    h(
      "p",
      { class: "notice" },
      "RoVibe peut continuer en arrière-plan : les agents poursuivent leur travail, et l'icône près de l'horloge rouvre la fenêtre. Quitter les arrête ; leurs conversations seront proposées à la reprise.",
    ),
    h("label", { class: "check" }, remember, "Ne plus me demander"),
    h(
      "div",
      { class: "actions" },
      h("button", { onclick: () => dialog.close() }, "Annuler"),
      h("button", { onclick: () => choose("quit") }, "Quitter"),
      h("button", { class: "primary", onclick: () => choose("hide") }, "Continuer en arrière-plan"),
    ),
  );
  dialog.showModal();
}
(window as unknown as { rovibeClosing: () => void }).rovibeClosing = askBeforeClosing;

const requests = h("div", { class: "requests" });

/** What agents are waiting on the user to allow, e.g. putting the game online. */
function renderRequests(current: State) {
  const answer = (id: number, allow: boolean) => run(() => api(`/api/approvals/${id}`, "POST", { allow }));

  requests.replaceChildren(
    current.update
      ? h(
          "div",
          { class: "request update" },
          h("span", {}, `RoVibe ${current.update} est disponible. L'installer redémarre la fenêtre ; les sessions en cours continuent.`),
          h("button", { class: "primary", onclick: () => run(() => api("/api/update", "POST")) }, "Installer et redémarrer"),
        )
      : "",
    current.pending
      ? h(
          "div",
          { class: "request update" },
          h(
            "span",
            {},
            `RoVibe ${current.pending} est installé. Tes sessions tournent encore sur la version précédente : la nouvelle prendra le relais quand elles seront fermées, ou tout de suite si tu redémarres.`,
          ),
          h(
            "button",
            {
              class: "primary",
              onclick: () => {
                if (ask("Redémarrer maintenant ? Les sessions en cours seront interrompues, et proposées à la reprise.")) {
                  void run(() => api("/api/host/restart", "POST"));
                }
              },
            },
            "Redémarrer maintenant",
          ),
        )
      : "",
    ...current.approvals.map((approval) => {
      const project = current.projects.find((candidate) => candidate.id === approval.project_id);
      return h(
        "div",
        { class: "request", role: "alert" },
        h("span", {}, `${approval.requester} demande à ${approval.request}${project ? ` (projet ${project.name})` : ""}.`),
        h("button", { class: "primary", onclick: () => answer(approval.id, true) }, "Autoriser"),
        h("button", { onclick: () => answer(approval.id, false) }, "Refuser"),
      );
    }),
  );
  requests.hidden = current.approvals.length === 0 && !current.update && !current.pending;
}

/** `grid` shows every session of the project side by side; `tabs` one at a
 *  time, at full size, with a strip to move between them. */
let layout: "grid" | "tabs" = localStorage.getItem("rovibe.layout") === "tabs" ? "tabs" : "grid";
/** The session shown in `tabs` layout, per project. */
const focused = new Map<string, string>();

/** The order the user gave the panes of a project, as session titles: ids
 *  change when a session is resumed, its title doesn't. */
function paneOrder(projectId: string): string[] {
  try {
    const saved = JSON.parse(localStorage.getItem(`rovibe.order.${projectId}`) ?? "[]");
    return Array.isArray(saved) ? saved.filter((title) => typeof title === "string") : [];
  } catch {
    return [];
  }
}

function ordered(sessions: Session[], projectId: string) {
  const order = paneOrder(projectId);
  const rank = (session: Session) => {
    const index = order.indexOf(session.title);
    return index >= 0 ? index : order.length + Number(session.id.slice(1));
  };
  return sessions.filter((session) => session.project_id === projectId).sort((a, b) => rank(a) - rank(b));
}

/** Title of the pane being dragged, while a drag is going on. */
let dragging: string | null = null;

/** Puts the dragged pane where another one is, and remembers it. */
function movePane(projectId: string, moved: string, target: string) {
  if (!state || moved === target) return;
  const titles = ordered(state.sessions, projectId).map((session) => session.title);
  const from = titles.indexOf(moved);
  const to = titles.indexOf(target);
  if (from < 0 || to < 0) return;
  titles.splice(from, 1);
  titles.splice(to, 0, moved);
  localStorage.setItem(`rovibe.order.${projectId}`, JSON.stringify(titles));
  render();
}

function gridColumns(count: number) {
  if (count <= 1) return 1;
  if (count <= 4) return 2;
  return 3;
}

const rail = h("aside", { class: "rail" });
const bar = h("header", { class: "bar" });
const grid = h("div", { class: "grid" });
const resumeBar = h("div", { class: "resume" });
const tabsBar = h("div", { class: "tabsbar", role: "tablist" });
const empty = h("div", { class: "empty" });

function renderRail(current: State) {
  const logo = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  logo.setAttribute("viewBox", "0 0 26 26");
  logo.innerHTML =
    '<path fill="#a78bfa" d="M13 1l5 3v6l-5 3-5-3V4z"/><path fill="#8b5cf6" d="M6.5 12.5l5 3v6l-5 3-5-3v-6z"/><path fill="#6d3fe0" d="M19.5 12.5l5 3v6l-5 3-5-3v-6z"/>';

  const todo = setupItems(current).filter((item) => item.needed && !item.ok).length;
  const tool = (glyph: keyof typeof ICONS, label: string, hint: string, action: () => void, note = "", current = false) =>
    h(
      "button",
      { class: "tool", title: hint, "aria-current": current ? "page" : false, onclick: action },
      icon(glyph),
      h("span", {}, label),
      note && h("small", {}, note),
    );

  rail.replaceChildren(
    h("div", { class: "brand", title: "Vibe Code Together in Roblox Studio." }, logo, "RoVibe"),
    h(
      "div",
      { class: "rail-head" },
      h("span", {}, "Projets"),
      h("button", { class: "quiet add", title: "Nouveau projet", "aria-label": "Nouveau projet", onclick: openProjectDialog }, icon("plus")),
    ),
    h(
      "nav",
      { class: "projects", "aria-label": "Projets" },
      ...current.projects.map((project) => {
        const live = current.sessions.filter((s) => s.project_id === project.id && !s.exited);
        const count = live.length;
        const waiting = live.filter((s) => s.status === "waiting").length;
        const dormant = current.dormant.filter((s) => s.project_id === project.id).length;
        return h(
          "button",
          { class: "project", "aria-current": String(project.id === selected && view === "project"), onclick: () => select(project.id) },
          h("span", { class: waiting > 0 ? "dot off" : count > 0 ? "dot on" : "dot" }),
          h(
            "span",
            { class: "project-text" },
            h("strong", {}, project.name),
            h(
              "small",
              { class: waiting > 0 ? "calls" : "" },
              waiting > 0
                ? waiting === 1
                  ? "1 agent attend ta réponse"
                  : `${waiting} agents attendent ta réponse`
                : dormant > 0 && count === 0
                  ? dormant === 1
                    ? "1 session à reprendre"
                    : `${dormant} sessions à reprendre`
                  : count === 0
                    ? "Aucun agent"
                    : count === 1
                      ? "1 agent actif"
                      : `${count} agents actifs`,
            ),
          ),
        );
      }),
    ),
    h(
      "div",
      { class: "rail-foot" },
      h("div", { class: "rail-head" }, h("span", {}, "Outils")),
      tool("bank", "Banque d'assets", "Modèles réutilisables et Creator Store", () => run(openBankDialog)),
      current.tools.sync &&
        tool(
          "plugin",
          "Plugin Studio",
          "Copie RoVibeStudio.rbxm dans le dossier Plugins de Roblox Studio",
          () => run(() => api("/api/plugin/install", "POST")),
          current.plugin_installed ? "mettre à jour" : "à installer",
        ),
      tool("settings", "Réglages", "Langue, modèles, Studio, agents isolés", () => run(openSettings), "", view === "settings"),
      tool("log", "Journal", "Ce que l'app a fait", () => run(openLogDialog)),
      tool(
        "setup",
        "Premiers pas",
        "Ce dont RoVibe a besoin sur ce PC, et ce qui manque",
        openSetupDialog,
        todo === 0 ? "" : todo === 1 ? "1 à régler" : `${todo} à régler`,
      ),
      h("div", { class: "version" }, `RoVibe ${current.version}`),
    ),
  );
}

function renderBar(current: State, project: Project) {
  const edits = current.studios.filter((studio) => studio.context === "edit");
  const bound = project.place_id != null || project.place_name != null;
  // Same rule as the server: a published place by its id, a local file by its name.
  const isBoundTo = (studio: Studio) =>
    project.place_id != null ? studio.place_id === project.place_id : studio.name === project.place_name;
  const linked = edits.filter((studio) => !bound || isBoundTo(studio));
  const playing = current.studios.some((studio) => studio.context === "server");

  const blocked = linked[0]?.blocked;
  const studioText =
    linked.length === 0
      ? "Studio non connecté"
      : blocked
        ? `${linked[0].name} : bloqué`
        : `${linked[0].name}${playing ? ", test en cours" : ""}${linked.length > 1 ? ` (+${linked.length - 1})` : ""}`;

  const binding = h(
    "select",
    {
      title: "Place Studio visée par les agents de ce projet",
      onchange: (event: Event) => {
        const chosen = edits[Number((event.target as HTMLSelectElement).value)];
        const body = !chosen
          ? { place_id: null, place_name: null }
          : chosen.place_id !== 0
            ? { place_id: chosen.place_id, place_name: null }
            : { place_id: null, place_name: chosen.name };
        void run(() => api(`/api/projects/${project.id}/bind`, "POST", body));
      },
    },
    h("option", { value: "" }, "Place : automatique"),
    ...edits.map((studio, index) =>
      h("option", { value: index, selected: bound && isBoundTo(studio) }, `Place : ${studio.name}`),
    ),
  );
  if (bound && !edits.some(isBoundTo)) {
    binding.append(
      h("option", { value: "closed", selected: true }, `Place : ${project.place_name ?? project.place_id} (fermée)`),
    );
  }

  const sync = (action: string) => run(() => api(`/api/projects/${project.id}/sync`, "POST", { action }));

  const live = current.sessions.filter((session) => session.project_id === project.id && !session.exited);
  const busy = live.filter((session) => session.status === "working").length;
  const calling = live.filter((session) => session.status === "waiting").length;
  const summary =
    live.length === 0
      ? "Aucun agent"
      : [
          live.length === 1 ? "1 agent" : tr(`${live.length} agents`),
          busy > 0 ? tr(`${busy} au travail`) : "",
          calling > 0 ? tr(`${calling} en attente de toi`) : "",
        ]
          .filter(Boolean)
          .join(" · ");

  bar.replaceChildren(
    h(
      "div",
      { class: "bar-row" },
      h(
        "div",
        { class: "identity" },
        h("h1", {}, project.name),
        h("span", { class: "path", title: project.path }, icon("folder"), h("span", {}, project.path)),
      ),
      h(
        "div",
        { class: "group", role: "group", "aria-label": "Roblox Studio" },
        h(
          "span",
          {
            class: blocked ? "pill alert" : "pill",
            title: blocked
              ? `Studio attend une réponse : ${blocked}. Tant qu'elle est ouverte, il ignore les touches, les clics et les tests.`
              : "Place Studio vue par les agents de ce projet",
          },
          h("span", { class: linked.length && !blocked ? "dot on" : "dot off" }),
          studioText,
        ),
        edits.length > 1 || bound ? binding : "",
        h(
          "button",
          {
            class: project.sync_running ? "pill sync" : "pill",
            title: project.sync_running ? "Arrêter le serveur de synchro" : "Démarrer le serveur de synchro",
            onclick: () => sync(project.sync_running ? "stop" : "start"),
          },
          h("span", { class: project.sync_running ? "dot on" : "dot off" }),
          !project.sync_running
            ? "Synchro arrêtée"
            : project.sync_branch
              ? "Synchro : branche d'un agent"
              : `Synchro : port ${project.sync_port}`,
        ),
        h(
          "button",
          {
            class: "pill accent",
            disabled: linked.length === 0,
            title: project.sync_branch
              ? "Studio montre la branche d'un agent : reconnecter le ramène au dossier du projet"
              : "Connecte Studio au serveur de synchro du projet",
            onclick: () => sync("connect"),
          },
          icon("link"),
          "Connecter",
        ),
        h(
          "button",
          {
            class: project.protected ? "pill guard on" : "pill guard",
            "aria-pressed": String(project.protected),
            title: project.protected
              ? "Projet protégé : Studio montre les changements avant chaque synchro, et un agent doit te demander avant de la connecter. Clique pour retirer la protection."
              : "Protéger ce projet : Studio demandera avant chaque synchro, et un agent devra te demander avant de la connecter. À activer pour un jeu en ligne.",
            onclick: () => run(() => api(`/api/projects/${project.id}/protect`, "POST", { protected: !project.protected })),
          },
          icon("lock"),
          project.protected ? "Protégé" : "Protéger",
        ),
      ),
      h("span", { class: "spacer" }),
      h(
        "div",
        { class: "group", role: "group", "aria-label": "Projet" },
        h(
          "button",
          { title: "Ce que les agents ont modifié depuis ta dernière relecture", onclick: () => run(() => openChangesDialog(project)) },
          icon("changes"),
          "Changements",
        ),
        h("button", { title: "Points de sauvegarde et retour en arrière", onclick: () => run(() => openHistoryDialog(project)) }, icon("history"), "Historique"),
        h(
          "button",
          {
            class: "publish",
            disabled: linked.length === 0,
            title: "Met la place ouverte dans Studio en ligne sur Roblox, par le raccourci de publication de Studio",
            onclick: () => {
              if (ask(`Publier « ${linked[0].name} » sur Roblox ? Les joueurs recevront cette version.`)) {
                void run(() => api(`/api/projects/${project.id}/publish`, "POST"));
              }
            },
          },
          icon("publish"),
          "Publier",
        ),
      ),
    ),
    h(
      "div",
      { class: "bar-row launch" },
      h("span", { class: "label" }, "Lancer"),
      h(
        "div",
        { class: "group" },
        h(
          "button",
          { class: "start claude", disabled: !current.tools.claude, title: current.tools.claude ? "Nouvelle session Claude Code" : "claude introuvable dans le PATH", onclick: () => newSession("claude") },
          icon("claude"),
          "Claude Code",
        ),
        h(
          "button",
          { class: "start", disabled: !current.tools.codex, title: current.tools.codex ? "Nouvelle session Codex" : "codex introuvable dans le PATH", onclick: () => newSession("codex") },
          icon("codex"),
          "Codex",
        ),
        h("button", { class: "start", title: "Un terminal dans le dossier du projet", onclick: () => newSession("shell") }, icon("shell"), "Terminal"),
      ),
      h("span", { class: "label" }, "avec"),
      h(
        "div",
        { class: "group options" },
        h(
          "select",
          {
            title: "Modèle de la prochaine session Claude Code",
            onchange: (event: Event) => {
              model = (event.target as HTMLSelectElement).value;
              localStorage.setItem("rovibe.model", model);
            },
          },
          ...[
            ["", "Modèle des réglages"],
            ["opus", "Opus"],
            ["sonnet", "Sonnet"],
            ["haiku", "Haiku"],
          ].map(([value, label]) => h("option", { value, selected: value === model }, label)),
        ),
        h(
          "label",
          { class: "check", title: "Les agents agissent sans demander de confirmation. Un point de sauvegarde est créé avant chaque session lancée ainsi." },
          h("input", {
            type: "checkbox",
            checked: skipPermissions,
            onchange: (event: Event) => {
              skipPermissions = (event.target as HTMLInputElement).checked;
              localStorage.setItem("rovibe.skip", skipPermissions ? "1" : "0");
            },
          }),
          "Sans confirmations",
        ),
        h(
          "label",
          {
            class: "check",
            title: current.isolation
              ? "L'agent ne voit que le dossier du projet. Il tourne dans une distribution WSL sans accès au reste du PC ; Codex garde son propre bac à sable tant qu'il n'y est pas installé."
              : "Codex seulement pour l'instant, dans son propre bac à sable. Pour la distribution WSL, lance scripts\\setup-isolation.ps1 une fois.",
          },
          h("input", {
            type: "checkbox",
            checked: isolated,
            onchange: (event: Event) => {
              isolated = (event.target as HTMLInputElement).checked;
              localStorage.setItem("rovibe.isolated", isolated ? "1" : "0");
            },
          }),
          "Isolé",
        ),
        h(
          "label",
          {
            class: "check",
            title: "Chaque nouvel agent travaille dans une copie du projet, sur sa propre branche git : plusieurs agents peuvent alors modifier les mêmes fichiers. Tu fusionnes leur travail depuis leur panneau.",
          },
          h("input", {
            type: "checkbox",
            checked: apart,
            onchange: (event: Event) => {
              apart = (event.target as HTMLInputElement).checked;
              localStorage.setItem("rovibe.apart", apart ? "1" : "0");
            },
          }),
          "Branche à part",
        ),
      ),
      h("span", { class: "spacer" }),
      h("span", { class: calling > 0 ? "summary calls" : "summary" }, summary),
      h(
        "div",
        { class: "segmented", role: "group", "aria-label": "Disposition des panneaux" },
        ...(
          [
            ["grid", "Tous les panneaux côte à côte"],
            ["tabs", "Un seul panneau à la fois, en grand"],
          ] as const
        ).map(([mode, hint]) =>
          h(
            "button",
            {
              "aria-pressed": String(layout === mode),
              title: hint,
              onclick: () => {
                layout = mode;
                localStorage.setItem("rovibe.layout", mode);
                render();
              },
            },
            icon(mode),
          ),
        ),
      ),
    ),
  );
}

function renderPanes(current: State, project: Project | undefined) {
  const alive = new Set(current.sessions.map((session) => session.id));
  for (const [id, pane] of panes) {
    if (!alive.has(id)) {
      pane.socket.close();
      pane.terminal.dispose();
      pane.element.remove();
      panes.delete(id);
    }
  }

  // Sessions of the previous run are offered, not restarted: each one starts
  // an agent and reloads a conversation. They wait in a strip of their own,
  // so the grid only holds what is running.
  const waiting = current.dormant.filter((session) => session.project_id === project?.id);
  resumeBar.hidden = waiting.length === 0;
  resumeBar.replaceChildren(
    h("span", { class: "label" }, waiting.length === 1 ? "Session interrompue" : "Sessions interrompues"),
    ...waiting.map((session) =>
      h(
        "span",
        { class: "resume-item", title: "Ouverte à la dernière fermeture de l'app. La reprendre relance l'agent sur sa conversation." },
        h("span", { class: "agent" }, icon("claude")),
        h("strong", {}, session.title),
        session.isolated ? h("small", {}, "isolée") : null,
        h("button", { class: "primary", onclick: () => run(() => api(`/api/dormant/${session.agent_id}`, "POST")) }, "Reprendre"),
        h("button", { class: "quiet", title: "Oublier cette session", onclick: () => run(() => api(`/api/dormant/${session.agent_id}`, "DELETE")) }, icon("close")),
      ),
    ),
    waiting.length > 1
      ? h(
          "button",
          {
            onclick: () =>
              run(async () => {
                for (const session of waiting) await api(`/api/dormant/${session.agent_id}`, "POST");
              }),
          },
          "Tout reprendre",
        )
      : "",
  );

  const mine = project ? ordered(current.sessions, project.id) : [];
  if (project && !mine.some((session) => session.id === focused.get(project.id))) {
    focused.set(project.id, mine[0]?.id ?? "");
  }
  const single = layout === "tabs" && mine.length > 1;

  let visible = 0;
  for (const session of current.sessions) {
    let pane = panes.get(session.id);
    if (!pane) {
      pane = createPane(session);
      panes.set(session.id, pane);
      grid.append(pane.element);
      pane.mount();
    }
    const index = mine.indexOf(session);
    const shown = index >= 0 && (!single || focused.get(session.project_id) === session.id);
    pane.element.hidden = !shown;
    pane.element.style.order = String(index);
    pane.element.classList.toggle("dead", session.exited);
    const status = session.exited ? "" : session.status;
    pane.element.dataset.status = status;
    const label = session.exited ? tr("terminée") : describeStatus(session);
    pane.state.textContent = session.isolated ? [label, tr("isolé")].filter(Boolean).join(", ") : label;
    pane.files.textContent = tr(
      session.files.length === 0 ? "" : session.files.length === 1 ? "1 fichier tenu" : `${session.files.length} fichiers tenus`,
    );
    pane.files.title = session.files.join("\n");
    pane.target.hidden = session.exited;
    if (shown) visible += 1;
  }

  tabsBar.hidden = !single;
  tabsBar.replaceChildren(
    ...mine.map((session) =>
      h(
        "button",
        {
          class: "tab",
          role: "tab",
          "aria-selected": String(focused.get(session.project_id) === session.id),
          "data-status": session.exited ? "" : session.status,
          title: describeStatus(session),
          onclick: () => {
            focused.set(session.project_id, session.id);
            render();
            panes.get(session.id)?.terminal.focus();
          },
        },
        h("span", { class: `agent ${session.kind}` }, icon(session.kind)),
        session.title,
        h("span", { class: "status-dot" }),
      ),
    ),
  );

  grid.style.gridTemplateColumns = `repeat(${gridColumns(visible)}, minmax(0, 1fr))`;

  if (!project) {
    empty.replaceChildren(
      h("h2", {}, "Vibe Code Together in Roblox Studio."),
      h("p", { class: "badges" }, h("span", {}, "Multi-Agent"), h("span", {}, "MCP"), h("span", {}, "Studio Sync")),
      h("p", {}, "Un projet est un dossier de code synchronisé avec une place Roblox Studio. Les agents y travaillent en parallèle."),
      h("div", { class: "actions" }, h("button", { class: "primary", onclick: openProjectDialog }, "Nouveau projet")),
    );
  } else if (visible === 0) {
    empty.replaceChildren(
      h("h2", {}, "Aucun agent sur ce projet"),
      h("p", {}, "Lance un ou plusieurs agents. Chacun reçoit les outils Studio du projet et travaille dans son dossier."),
      h(
        "div",
        { class: "actions" },
        h("button", { class: "primary", disabled: !current.tools.claude, onclick: () => newSession("claude") }, "Lancer Claude Code"),
        h("button", { onclick: () => newSession("shell") }, "Ouvrir un terminal"),
      ),
    );
  }
  empty.hidden = Boolean(project) && visible > 0;
}

function render() {
  if (!state) return;
  if (!state.projects.some((project) => project.id === selected)) {
    selected = state.projects[0]?.id ?? null;
  }
  const project = state.projects.find((candidate) => candidate.id === selected);

  const inSettings = view === "settings";
  renderRail(state);
  if (project) renderBar(state, project);
  bar.hidden = !project || inSettings;
  renderRequests(state);
  renderPanes(state, project);
  renderComposer();
  // The settings take the place of the project; its terminals stay as they
  // are behind, and come back untouched.
  settingsPage.hidden = !inSettings;
  grid.hidden = inSettings;
  if (inSettings) {
    resumeBar.hidden = tabsBar.hidden = composer.hidden = true;
    if (settingsStatus) settingsStatus(state);
    // After a reload, the page is asked for again but not built yet.
    else if (!settingsPage.hasChildNodes()) void openSettings();
  }
  if (setupOpen) drawSetup();
  // A fresh install starts here, once.
  if (state.projects.length === 0 && !localStorage.getItem("rovibe.welcomed") && !dialog.open) {
    localStorage.setItem("rovibe.welcomed", "1");
    openSetupDialog();
  }
}

async function refresh() {
  try {
    state = await api<State>("/api/state");
    announceChanges(state);
    render();
  } catch (error) {
    toast(`Serveur injoignable : ${(error as Error).message}`, true);
  }
}

function listen() {
  const socket = new WebSocket(socketUrl("/ws/events"));
  // A busy agent sends changes in bursts: one refresh for each burst.
  let pending = 0;
  socket.onmessage = () => {
    clearTimeout(pending);
    pending = window.setTimeout(() => void refresh(), 60);
  };
  socket.onclose = () => setTimeout(listen, 1500);
}

// For the automated tests of the interface: what a terminal shows, which
// lives on a canvas and nowhere in the page.
(window as unknown as { rovibeTest: unknown }).rovibeTest = {
  screen(title: string) {
    const session = state?.sessions.find((candidate) => candidate.title === title);
    const terminal = session && panes.get(session.id)?.terminal;
    if (!terminal) return null;
    const buffer = terminal.buffer.active;
    const lines: string[] = [];
    for (let row = 0; row < buffer.length; row++) lines.push(buffer.getLine(row)?.translateToString(true) ?? "");
    return { cols: terminal.cols, rows: terminal.rows, text: lines.join("\n").trimEnd() };
  },
};

grid.append(empty);
app.append(h("div", { class: "shell" }, rail, h("main", { class: "main" }, bar, requests, settingsPage, resumeBar, tabsBar, grid, composer)), dialog, toasts);
void refresh();
listen();
