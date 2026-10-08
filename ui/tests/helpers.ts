import path from "node:path";

import { expect, type Page } from "@playwright/test";

import { dataDir } from "../playwright.config";

/** Calls the server's API from the page, with the page's own token. */
export async function api<T = unknown>(page: Page, route: string, method = "GET", body?: unknown): Promise<T> {
  return page.evaluate(
    async ({ route, method, body }) => {
      const token = document.querySelector<HTMLMetaElement>('meta[name="rovibe-token"]')!.content;
      const response = await fetch(route, {
        method,
        headers: { Authorization: `Bearer ${token}`, "Content-Type": "application/json" },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
      return response.json().catch(() => ({}));
    },
    { route, method, body },
  );
}

interface State {
  projects: { id: string; name: string; protected: boolean }[];
  sessions: { id: string; title: string; exited: boolean }[];
}

/** Opens the app on a project that exists, creating it the first time. */
export async function openProject(page: Page, name = "Essai") {
  await page.goto("/");
  let state = await api<State>(page, "/api/state");
  if (!state.projects.some((project) => project.name === name)) {
    await api(page, "/api/projects", "POST", { name, path: path.join(dataDir, "projets", name) });
    state = await api<State>(page, "/api/state");
  }
  // A fresh install greets with the checklist; the scenarios start past it.
  await page.evaluate(() => localStorage.setItem("rovibe.welcomed", "1"));
  await page.reload();
  await page.locator(".project", { hasText: name }).click();
  return state.projects.find((project) => project.name === name)!;
}

/** Makes sure exactly `count` terminals run in the project. */
export async function withTerminals(page: Page, projectId: string, count: number) {
  const state = await api<State>(page, "/api/state");
  for (const session of state.sessions) await api(page, `/api/sessions/${session.id}`, "DELETE");
  for (let index = 0; index < count; index++) await api(page, "/api/sessions", "POST", { project_id: projectId, kind: "shell" });
  await expect(page.locator(".pane:not([hidden])")).toHaveCount(count);
  // Their size settles once the shells have answered.
  await page.waitForTimeout(1500);
}

export function screen(page: Page, title: string) {
  return page.evaluate(
    (title) => (window as unknown as { rovibeTest: { screen(title: string): { cols: number; rows: number; text: string } | null } }).rovibeTest.screen(title),
    title,
  );
}

/** Text a French speaker would recognise, left in a page meant to be English. */
export async function frenchLeftIn(page: Page, root: string) {
  return page.evaluate((root) => {
    const french = /[éèêàçùîôâû«»]|\b(les|des|une|pour|avec|dans|aucun|projet|fichier|dossier)\b/i;
    const found = new Set<string>();
    const element = document.querySelector(root)!;
    const walker = document.createTreeWalker(element, NodeFilter.SHOW_TEXT);
    while (walker.nextNode()) {
      const node = walker.currentNode;
      const text = node.textContent?.trim() ?? "";
      // Terminals, the journal and names typed by the user are not the interface's words.
      if (text && french.test(text) && !node.parentElement?.closest(".xterm, .log, .project, .title, option[value='fr']")) found.add(text.slice(0, 80));
    }
    for (const part of element.querySelectorAll("[title], [placeholder], [aria-label]")) {
      for (const name of ["title", "placeholder", "aria-label"]) {
        const value = part.getAttribute(name);
        if (value && french.test(value)) found.add(`${name}: ${value.slice(0, 80)}`);
      }
    }
    return [...found];
  }, root);
}
