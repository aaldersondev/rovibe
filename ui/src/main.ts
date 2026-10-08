import "@fontsource-variable/bricolage-grotesque";
import "@xterm/xterm/css/xterm.css";
import "./style.css";

import { FitAddon } from "@xterm/addon-fit";
import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";

type Kind = "claude" | "codex" | "shell";

interface Project {
  id: string;
  name: string;
  path: string;
  sync_port: number;
  place_id: number | null;
  place_name: string | null;
  sync_running: boolean;
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
  tools: { claude: boolean; codex: boolean; sync: boolean };
  checkers: { selene: boolean; luau_lsp: boolean };
  approvals: { id: number; project_id: string; requester: string; request: string }[];
  /** Version of an update waiting to be installed, if any. */
  update: string | null;
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

const token = document.querySelector<HTMLMetaElement>('meta[name="essaim-token"]')!.content;
const app = document.getElementById("app")!;
const panes = new Map<string, Pane>();

let state: State | null = null;
let selected = localStorage.getItem("essaim.project");
let skipPermissions = localStorage.getItem("essaim.skip") === "1";
let isolated = localStorage.getItem("essaim.isolated") === "1";

type Child = Node | string | null | false;

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
      element.setAttribute(key, String(value));
    }
  }
  for (const child of children) {
    if (child) element.append(child);
  }
  return element;
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

function createPane(session: Session): Pane {
  const terminal = new Terminal({
    fontFamily: getComputedStyle(document.documentElement).getPropertyValue("--mono"),
    fontSize: 13,
    cursorBlink: true,
    scrollback: 8000,
    allowProposedApi: true,
    theme: { background: "#0a1319", foreground: "#e4edf0", cursor: "#f2b33d", selectionBackground: "#2a4654" },
  });
  const fit = new FitAddon();
  terminal.loadAddon(fit);

  const body = h("div", { class: "pane-body" });
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
      { class: "pane-head" },
      h("span", { class: `cell ${session.kind}` }),
      h("span", { class: "title" }, session.title),
      stateLabel,
      filesLabel,
      h("span", { class: "spacer" }),
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
        "⤢",
      ),
      h("button", { class: "quiet", title: "Fermer la session", onclick: () => closeSession(session.id) }, "✕"),
    ),
    body,
  );

  const socket = new WebSocket(socketUrl(`/ws/pty/${session.id}`));
  socket.binaryType = "arraybuffer";
  socket.onmessage = (event) => terminal.write(new Uint8Array(event.data as ArrayBuffer));

  const send = (message: object) => {
    if (socket.readyState === WebSocket.OPEN) socket.send(JSON.stringify(message));
  };

  terminal.onData((data) => send({ t: "i", d: data }));
  terminal.onResize(({ cols, rows }) => send({ t: "r", cols, rows }));
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

  // A hidden pane has no size; fitting it would collapse the terminal.
  const refit = () => {
    if (body.clientWidth > 0 && body.clientHeight > 0) fit.fit();
  };

  // xterm measures its character cell when it opens. Opened while detached it
  // measures nothing, every later fit is a no-op, and the terminal stays at
  // 80x24 inside a much larger pane.
  const mount = () => {
    terminal.open(body);
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

  // Sent even when the size didn't change: the process starts at a default
  // size and has to learn the real one.
  socket.onopen = () => send({ t: "r", cols: terminal.cols, rows: terminal.rows });

  return { mount, element, terminal, socket, state: stateLabel, files: filesLabel, target };
}

async function closeSession(id: string) {
  const session = state?.sessions.find((candidate) => candidate.id === id);
  if (session && !session.exited && !confirm(`Arrêter « ${session.title} » ?`)) return;
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
    });
    created = session.id;
  });
  // The pane only exists once `run` has refreshed the state.
  if (created) panes.get(created)?.terminal.focus();
}

const dialog = h("dialog");

function openProjectDialog() {
  const name = h("input", { name: "name", required: true, autocomplete: "off" });
  const path = h("input", { name: "path", autocomplete: "off", placeholder: "Documents\\Essaim\\<nom>" });
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
      h("p", { class: "notice" }, "Ce projet n'a pas son propre dépôt git : Essaim ne gère pas son historique."),
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
                    if (confirm(`Revenir à « ${commit.subject} » ? L'état actuel sera sauvegardé avant.`)) {
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
  thumb: boolean;
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
  dialog.classList.add("wide");
  dialog.addEventListener("close", () => dialog.classList.remove("wide"), { once: true });
  dialog.showModal();
  text.scrollTop = text.scrollHeight;
}

async function openBankDialog() {
  const bank = await api<{ dir: string; assets: BankAsset[] }>("/api/assets");
  const filter = h("input", { type: "search", placeholder: "Filtrer par nom ou tag", autocomplete: "off" });
  const list = h("ul", { class: "bank" });

  const draw = () => {
    const words = filter.value.toLowerCase().split(/\s+/).filter(Boolean);
    const shown = bank.assets.filter((asset) => {
      const haystack = `${asset.name} ${asset.tags.join(" ")} ${asset.class}`.toLowerCase();
      return words.every((word) => haystack.includes(word));
    });

    list.replaceChildren(
      ...shown.map((asset) =>
        h(
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
                asset.class || "fichier ajouté à la main",
                asset.instances ? `${asset.instances} instances` : "",
                `${Math.max(1, Math.round(asset.bytes / 1024))} Ko`,
                asset.tags.join(", "),
              ]
                .filter(Boolean)
                .join(", "),
            ),
            h("code", {}, `bank:${asset.id}`),
          ),
          h(
            "button",
            {
              class: "quiet",
              title: "Supprimer de la banque",
              onclick: async () => {
                if (!confirm(`Supprimer « ${asset.name} » de la banque ?`)) return;
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
        ),
      ),
    );
    if (shown.length === 0) {
      list.append(
        h(
          "li",
          { class: "notice" },
          bank.assets.length === 0
            ? "La banque est vide. Demande à un agent « enregistre Workspace.MonModele dans la banque », ou dépose des fichiers .rbxm dans le dossier ci-dessous."
            : "Aucun asset ne correspond.",
        ),
      );
    }
  };

  filter.addEventListener("input", draw);
  draw();

  dialog.replaceChildren(
    h("h2", {}, "Banque d'assets"),
    h(
      "p",
      { class: "notice" },
      "Modèles réutilisables d'un projet à l'autre. Les agents les trouvent avec asset_search et les posent avec asset_insert, tout comme les assets gratuits du Creator Store.",
    ),
    h("label", {}, filter),
    list,
    h("p", { class: "notice" }, `Dossier : ${bank.dir}`),
    h("div", { class: "actions" }, h("button", { onclick: () => dialog.close() }, "Fermer")),
  );
  dialog.showModal();
}

function select(id: string) {
  selected = id;
  localStorage.setItem("essaim.project", id);
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
  return session.detail && session.status !== "idle" ? `${label} : ${session.detail}` : label;
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
      toast(`${session.title} ${STATUS_LABELS[session.status]}`);
      if (!document.hasFocus()) unseen += 1;
    }
  }
  document.title = unseen > 0 ? `(${unseen}) Essaim` : "Essaim";
}

window.addEventListener("focus", () => {
  unseen = 0;
  document.title = "Essaim";
});

interface Prompt {
  name: string;
  text: string;
}

const prompts = new Map<string, Prompt[]>();
const composer = h("form", { class: "composer" });
const draft = h("textarea", {
  rows: 2,
  placeholder: "Consigne à envoyer aux agents cochés. Entrée pour envoyer, Maj+Entrée pour une nouvelle ligne.",
});
draft.addEventListener("keydown", (event) => {
  if (event.key === "Enter" && !event.shiftKey) {
    event.preventDefault();
    broadcast();
  }
});
draft.addEventListener("input", () => renderComposer());

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

  composer.replaceChildren(
    draft,
    h(
      "div",
      { class: "composer-side" },
      h(
        "button",
        { class: "primary", type: "submit", disabled: count === 0 || !draft.value.trim() },
        count === 0 ? "Aucun agent coché" : count === 1 ? "Envoyer à 1 agent" : `Envoyer aux ${count} agents`,
      ),
      h(
        "div",
        { class: "composer-saved" },
        picker,
        current >= 0
          ? h(
              "button",
              {
                type: "button",
                class: "quiet",
                onclick: () => {
                  if (confirm(`Supprimer la consigne « ${saved[current].name} » ?`)) {
                    void savePrompts(projectId, saved.filter((_, index) => index !== current));
                  }
                },
              },
              "Supprimer",
            )
          : h(
              "button",
              {
                type: "button",
                class: "quiet",
                disabled: !draft.value.trim(),
                onclick: () => {
                  const name = prompt("Nom de la consigne");
                  if (name?.trim()) void savePrompts(projectId, [...saved, { name: name.trim(), text: draft.value.trim() }]);
                },
              },
              "Enregistrer",
            ),
      ),
    ),
  );
}

composer.addEventListener("submit", (event) => {
  event.preventDefault();
  broadcast();
});

const requests = h("div", { class: "requests" });

/** What agents are waiting on the user to allow, e.g. putting the game online. */
function renderRequests(current: State) {
  const answer = (id: number, allow: boolean) => run(() => api(`/api/approvals/${id}`, "POST", { allow }));

  requests.replaceChildren(
    current.update
      ? h(
          "div",
          { class: "request update" },
          h("span", {}, `Essaim ${current.update} est disponible. L'installer redémarre l'app ; tes sessions Claude Code seront proposées à la reprise.`),
          h("button", { class: "primary", onclick: () => run(() => api("/api/update", "POST")) }, "Installer et redémarrer"),
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
  requests.hidden = current.approvals.length === 0 && !current.update;
}

function gridColumns(count: number) {
  if (count <= 1) return 1;
  if (count <= 4) return 2;
  return 3;
}

const rail = h("aside", { class: "rail" });
const bar = h("header", { class: "bar" });
const grid = h("div", { class: "grid" });
const empty = h("div", { class: "empty" });

function renderRail(current: State) {
  const logo = document.createElementNS("http://www.w3.org/2000/svg", "svg");
  logo.setAttribute("viewBox", "0 0 26 26");
  logo.innerHTML =
    '<path fill="#f2b33d" d="M13 1l5 3v6l-5 3-5-3V4z"/><path fill="#5fd3a6" d="M6.5 12.5l5 3v6l-5 3-5-3v-6z"/><path fill="#a99cf5" d="M19.5 12.5l5 3v6l-5 3-5-3v-6z"/>';

  rail.replaceChildren(
    h("div", { class: "brand" }, logo, "Essaim"),
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
          { class: "project", "aria-current": String(project.id === selected), onclick: () => select(project.id) },
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
        );
      }),
    ),
    h(
      "div",
      { class: "rail-foot" },
      !current.tools.sync && h("div", { class: "notice" }, "Serveur de synchro introuvable : compile vendor/sync."),
      !(current.checkers.selene && current.checkers.luau_lsp) &&
        h("div", { class: "notice" }, "Vérification du code incomplète : lance scripts\\get-tools.ps1 pour installer selene et luau-lsp."),
      current.tools.sync &&
        h(
          "button",
          {
            title: "Copie EssaimSync.rbxm dans le dossier Plugins de Roblox Studio",
            onclick: () => run(() => api("/api/plugin/install", "POST")),
          },
          current.plugin_installed ? "Mettre à jour le plugin Studio" : "Installer le plugin Studio",
        ),
      h("button", { onclick: () => run(openBankDialog) }, "Banque d'assets"),
      h("button", { class: "quiet", onclick: () => run(openLogDialog) }, "Journal"),
      h("button", { class: "primary", onclick: openProjectDialog }, "Nouveau projet"),
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

  bar.replaceChildren(
    h("h1", {}, project.name),
    h("span", { class: "path", title: project.path }, project.path),
    h(
      "span",
      {
        class: blocked ? "pill alert" : "pill",
        title: blocked ? `Studio attend une réponse : ${blocked}. Tant qu'elle est ouverte, il ignore les touches, les clics et les tests.` : "",
      },
      h("span", { class: linked.length && !blocked ? "dot on" : "dot off" }),
      studioText,
    ),
    edits.length > 1 || bound ? binding : "",
    h(
      "button",
      {
        class: "pill",
        title: project.sync_running ? "Arrêter le serveur de synchro" : "Démarrer le serveur de synchro",
        onclick: () => sync(project.sync_running ? "stop" : "start"),
      },
      h("span", { class: project.sync_running ? "dot on" : "dot off" }),
      project.sync_running ? `Synchro : port ${project.sync_port}` : "Synchro arrêtée",
    ),
    h(
      "button",
      { disabled: linked.length === 0, title: "Connecte Studio au serveur de synchro du projet", onclick: () => sync("connect") },
      "Connecter Studio",
    ),
    h("button", { title: "Points de sauvegarde et retour en arrière", onclick: () => run(() => openHistoryDialog(project)) }, "Historique"),
    h(
      "button",
      {
        disabled: linked.length === 0,
        title: "Met la place ouverte dans Studio en ligne sur Roblox, par le raccourci de publication de Studio",
        onclick: () => {
          if (confirm(`Publier « ${linked[0].name} » sur Roblox ? Les joueurs recevront cette version.`)) {
            void run(() => api(`/api/projects/${project.id}/publish`, "POST"));
          }
        },
      },
      "Publier",
    ),
    h("span", { class: "spacer" }),
    h(
      "label",
      { class: "check", title: "Les agents agissent sans demander de confirmation. Un point de sauvegarde est créé avant chaque session lancée ainsi." },
      h("input", {
        type: "checkbox",
        checked: skipPermissions,
        onchange: (event: Event) => {
          skipPermissions = (event.target as HTMLInputElement).checked;
          localStorage.setItem("essaim.skip", skipPermissions ? "1" : "0");
        },
      }),
      "Sans confirmations",
    ),
    h(
      "label",
      {
        class: "check",
        title: current.isolation
          ? "L'agent ne voit que le dossier du projet. Claude Code tourne dans une distribution WSL sans accès au reste du PC ; Codex dans son propre bac à sable."
          : "Codex seulement pour l'instant. Pour Claude Code, lance scripts\\setup-isolation.ps1 une fois.",
      },
      h("input", {
        type: "checkbox",
        checked: isolated,
        onchange: (event: Event) => {
          isolated = (event.target as HTMLInputElement).checked;
          localStorage.setItem("essaim.isolated", isolated ? "1" : "0");
        },
      }),
      "Isolé",
    ),
    h("button", { disabled: !current.tools.claude, title: current.tools.claude ? "" : "claude introuvable dans le PATH", onclick: () => newSession("claude") }, "+ Claude Code"),
    h("button", { disabled: !current.tools.codex, title: current.tools.codex ? "" : "codex introuvable dans le PATH", onclick: () => newSession("codex") }, "+ Codex"),
    h("button", { onclick: () => newSession("shell") }, "+ Terminal"),
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
  // an agent and reloads a conversation.
  for (const card of grid.querySelectorAll(".dormant")) card.remove();
  const waiting = current.dormant.filter((session) => session.project_id === project?.id);
  for (const session of waiting) {
    grid.append(
      h(
        "section",
        { class: "pane dormant" },
        h(
          "header",
          { class: "pane-head" },
          h("span", { class: "cell" }),
          h("span", { class: "title" }, session.title),
          h("span", { class: "state" }, session.isolated ? "interrompue, isolée" : "interrompue"),
        ),
        h(
          "div",
          { class: "dormant-body" },
          h("p", {}, "Cette session était ouverte à la dernière fermeture de l'app. La reprendre relance l'agent sur sa conversation."),
          h(
            "div",
            { class: "actions" },
            h(
              "button",
              { class: "primary", onclick: () => run(() => api(`/api/dormant/${session.agent_id}`, "POST")) },
              "Reprendre",
            ),
            h("button", { onclick: () => run(() => api(`/api/dormant/${session.agent_id}`, "DELETE")) }, "Oublier"),
          ),
        ),
      ),
    );
  }

  let visible = waiting.length;
  for (const session of current.sessions) {
    let pane = panes.get(session.id);
    if (!pane) {
      pane = createPane(session);
      panes.set(session.id, pane);
      grid.append(pane.element);
      pane.mount();
    }
    const shown = session.project_id === project?.id;
    pane.element.hidden = !shown;
    pane.element.classList.toggle("dead", session.exited);
    const status = session.exited ? "" : session.status;
    pane.element.dataset.status = status;
    const label = session.exited ? "terminée" : describeStatus(session);
    pane.state.textContent = session.isolated ? [label, "isolé"].filter(Boolean).join(", ") : label;
    pane.files.textContent =
      session.files.length === 0 ? "" : session.files.length === 1 ? "1 fichier tenu" : `${session.files.length} fichiers tenus`;
    pane.files.title = session.files.join("\n");
    pane.target.hidden = session.exited;
    if (shown) visible += 1;
  }

  grid.style.gridTemplateColumns = `repeat(${gridColumns(visible)}, minmax(0, 1fr))`;

  if (!project) {
    empty.replaceChildren(
      h("h2", {}, "Aucun projet"),
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

  renderRail(state);
  if (project) renderBar(state, project);
  bar.hidden = !project;
  renderRequests(state);
  renderPanes(state, project);
  renderComposer();
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
  socket.onmessage = () => void refresh();
  socket.onclose = () => setTimeout(listen, 1500);
}

grid.append(empty);
app.append(h("div", { class: "shell" }, rail, h("main", { class: "main" }, bar, requests, grid, composer)), dialog, toasts);
void refresh();
listen();
