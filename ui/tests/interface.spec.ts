import fs from "node:fs";
import path from "node:path";

import { expect, test, type Page } from "@playwright/test";

import { dataDir } from "../playwright.config";

import { api, frenchLeftIn, openProject, screen, withTerminals } from "./helpers";

// Each scenario below is a defect that reached a user once, or the heart of
// a feature that would be noticed late if it broke.

/** Where every visible pane and its terminal sit on the page. */
function boxes(page: Page) {
  return page.evaluate(() =>
    [...document.querySelectorAll<HTMLElement>(".pane:not([hidden])")].map((pane) => {
      const box = (element: Element | null) => {
        const rect = element?.getBoundingClientRect();
        return rect ? { left: Math.round(rect.left), top: Math.round(rect.top), right: Math.round(rect.right), bottom: Math.round(rect.bottom) } : null;
      };
      return {
        title: pane.querySelector(".title")?.textContent,
        pane: box(pane)!,
        head: box(pane.querySelector(".pane-head"))!,
        close: box(pane.querySelector(".pane-head button:last-child"))!,
        body: box(pane.querySelector(".pane-body"))!,
        terminal: box(pane.querySelector(".xterm-screen")),
      };
    }),
  );
}

test("with several sessions, each terminal stays inside its pane and nothing moves", async ({ page }) => {
  const project = await openProject(page);
  await withTerminals(page, project.id, 4);

  const first = await boxes(page);
  expect(first).toHaveLength(4);
  for (const pane of first) {
    // A terminal wider than its pane once stretched the header and pushed
    // the close button out of reach.
    expect(pane.terminal, `${pane.title} a un terminal`).not.toBeNull();
    expect(pane.terminal!.right).toBeLessThanOrEqual(pane.body.right + 1);
    expect(pane.terminal!.bottom).toBeLessThanOrEqual(pane.body.bottom + 1);
    expect(pane.head.right).toBeLessThanOrEqual(pane.pane.right);
    expect(pane.close.right).toBeLessThanOrEqual(pane.pane.right);
    expect(pane.close.left).toBeGreaterThan(pane.pane.left);
  }
  // Panes once resized each other in a loop.
  await page.waitForTimeout(2000);
  expect(await boxes(page)).toEqual(first);
});

test("the terminals take the room, not the command bar", async ({ page }) => {
  const project = await openProject(page);
  await withTerminals(page, project.id, 1);

  const sizes = await page.evaluate(() => ({
    grid: document.querySelector(".grid")!.getBoundingClientRect().height,
    composer: document.querySelector(".composer")!.getBoundingClientRect().height,
    page: innerHeight,
  }));
  // The command bar once took the whole height when no banner was shown.
  expect(sizes.composer).toBeLessThan(170);
  expect(sizes.grid).toBeGreaterThan(sizes.page * 0.5);
});

test("a terminal shows its shell, and still does after the window is opened again", async ({ page }) => {
  const project = await openProject(page);
  await withTerminals(page, project.id, 1);
  const title = (await boxes(page))[0].title!;

  await expect.poll(async () => (await screen(page, title))?.text ?? "", { timeout: 20_000 }).toContain("PS ");
  const before = await screen(page, title);

  await page.reload();
  await page.locator(".project", { hasText: "Essai" }).click();
  await expect.poll(async () => (await screen(page, title))?.text ?? "", { timeout: 20_000 }).toContain("PS ");
  const after = await screen(page, title);
  // Replayed at the size it was drawn for, then fitted: same width, and the
  // prompt on one line rather than cut in two.
  expect(after!.cols).toBe(before!.cols);
  expect(after!.text.split("\n").filter((line) => line.includes("PS "))).toHaveLength(1);
});

test("panes are reordered by dragging, and shown one at a time with tabs", async ({ page }) => {
  const project = await openProject(page);
  await withTerminals(page, project.id, 3);
  const order = async () =>
    page.evaluate(() =>
      [...document.querySelectorAll<HTMLElement>(".pane:not([hidden])")]
        .sort((a, b) => Number(a.style.order) - Number(b.style.order))
        .map((pane) => pane.querySelector(".title")!.textContent),
    );
  const [a, b, c] = await order();

  await page.locator(".pane", { hasText: c! }).locator(".pane-head").dragTo(page.locator(".pane", { hasText: a! }).locator(".pane-body"));
  expect(await order()).toEqual([c, a, b]);
  await page.reload();
  await page.locator(".project", { hasText: "Essai" }).click();
  expect(await order()).toEqual([c, a, b]);

  await page.locator(".segmented button").nth(1).click();
  await expect(page.locator(".pane:not([hidden])")).toHaveCount(1);
  await expect(page.locator(".tabsbar .tab")).toHaveText([c!, a!, b!]);
  await page.locator(".tabsbar .tab", { hasText: b! }).click();
  await expect(page.locator(".pane:not([hidden]) .title")).toHaveText(b!);
  await page.locator(".segmented button").nth(0).click();
  await expect(page.locator(".pane:not([hidden])")).toHaveCount(3);
});

test("settings are saved as they change, and a refused value blocks nothing", async ({ page }) => {
  await openProject(page);
  await page.locator(".rail .tool", { hasText: "Réglages" }).click();
  await expect(page.locator(".settings")).toBeVisible();
  await expect(page.locator(".grid")).toBeHidden();
  const row = (name: string) => page.locator(".setting", { has: page.locator("strong", { hasText: name }) });
  const stored = () => api<Record<string, string>>(page, "/api/settings");

  await row("Notifications Windows").locator("select").selectOption("waiting");
  await expect.poll(async () => (await stored()).notifications).toBe("waiting");
  await expect(page.locator(".settings .saved")).toHaveClass(/on/);

  // A key the server doesn't know: the error is shown, the field goes back,
  // and the next change is saved all the same.
  const shortcut = row("Raccourci").locator("input");
  await shortcut.fill("banane+p");
  await shortcut.blur();
  await expect(page.locator(".toast.error")).toContainText("banane");
  await expect(shortcut).toHaveValue("");
  await row("Modèle de Codex").locator("input").fill("un-modele");
  await row("Modèle de Codex").locator("input").blur();
  await expect.poll(async () => (await stored()).codex_model).toBe("un-modele");

  await page.locator(".settings-head button").click();
  await expect(page.locator(".settings")).toBeHidden();
  await expect(page.locator(".grid")).toBeVisible();
});

test("a project is protected and unprotected from its header", async ({ page }) => {
  await openProject(page);
  const guard = page.locator("button.guard");
  await expect(guard).toHaveAttribute("aria-pressed", "false");
  await guard.click();
  await expect(guard).toHaveAttribute("aria-pressed", "true");
  const state = await api<{ projects: { name: string; protected: boolean }[] }>(page, "/api/state");
  expect(state.projects.find((project) => project.name === "Essai")!.protected).toBe(true);
  await guard.click();
  await expect(guard).toHaveAttribute("aria-pressed", "false");
});

test("closing the window with sessions running asks what was meant", async ({ page }) => {
  const project = await openProject(page);
  await withTerminals(page, project.id, 2);
  await page.evaluate(() => (window as unknown as { rovibeClosing(): void }).rovibeClosing());
  const dialog = page.locator("dialog[open]");
  await expect(dialog.locator("h2")).toHaveText("2 sessions sont en cours");
  await expect(dialog.locator(".actions button")).toHaveText(["Annuler", "Quitter", "Continuer en arrière-plan"]);
  await dialog.locator("button", { hasText: "Annuler" }).click();
  await expect(dialog).toBeHidden();
});

test("the checklist says what is in place and what is missing", async ({ page }) => {
  await openProject(page);
  await page.locator(".rail .tool", { hasText: "Premiers pas" }).click();
  const items = page.locator("dialog[open] .setup li");
  await expect(items).toHaveCount(8);
  await expect(items.filter({ hasText: "Git" })).toHaveClass(/ok/);
});

test("in English, no French is left on the screens a user meets first", async ({ page }) => {
  const project = await openProject(page);
  await withTerminals(page, project.id, 2);
  await page.evaluate(() => localStorage.setItem("rovibe.lang", "en"));
  await page.reload();
  await page.locator(".project", { hasText: "Essai" }).click();
  await expect(page.locator(".rail .tool", { hasText: "Settings" })).toBeVisible();
  expect(await frenchLeftIn(page, "body")).toEqual([]);

  await page.locator(".rail .tool", { hasText: "Settings" }).click();
  await expect(page.locator(".settings-section")).toHaveCount(5);
  expect(await frenchLeftIn(page, ".settings")).toEqual([]);
  await page.locator(".settings-head button").click();

  for (const tool of ["Getting started", "Asset bank"]) {
    await page.locator(".rail .tool", { hasText: tool }).click();
    await expect(page.locator("dialog[open] h2")).toHaveText(tool);
    expect(await frenchLeftIn(page, "dialog[open]"), tool).toEqual([]);
    await page.evaluate(() => document.querySelector("dialog")!.close());
  }
  await page.locator(".rail-head .add").click();
  expect(await frenchLeftIn(page, "dialog[open]"), "New project").toEqual([]);
  await page.evaluate(() => {
    document.querySelector("dialog")!.close();
    localStorage.removeItem("rovibe.lang");
  });
});

test("an agent's branch is read before it is merged", async ({ page }) => {
  const project = await openProject(page, "Branches");
  const state = await api<{ tools: { claude: boolean; git: boolean } }>(page, "/api/state");
  // Needs a real agent to give a branch to: where none is installed, the
  // server's own tests cover what this window shows.
  test.skip(!state.tools.claude || !state.tools.git, "claude ou git absent");

  await withTerminals(page, project.id, 0);
  const session = await api<{ id: string; title: string; branch: string }>(page, "/api/sessions", "POST", {
    project_id: project.id,
    kind: "claude",
    worktree: true,
  });
  const own = path.join(dataDir, "worktrees", project.id, session.branch.split("/")[1]);
  fs.writeFileSync(path.join(own, "src", "shared", "Ajout.luau"), "return \"depuis la branche\"\n");

  const pane = page.locator(".pane", { hasText: session.title });
  await expect(pane.locator(".own-branch")).toBeVisible();
  await pane.locator("button.merge").click();
  const dialog = page.locator("dialog[open]");
  await expect(dialog.locator("h2")).toHaveText(`Travail de ${session.title}`);
  await expect(dialog.locator(".changes li")).toHaveCount(1);
  await expect(dialog.locator(".changes li")).toContainText("src/shared/Ajout.luau");
  await expect(dialog.locator(".diff")).toContainText("depuis la branche");

  await dialog.locator("button.primary").click();
  await expect(page.locator(".toast").last()).toContainText("fusionné");
  expect(fs.existsSync(path.join(dataDir, "projets", "Branches", "src", "shared", "Ajout.luau"))).toBe(true);
  await api(page, `/api/sessions/${session.id}`, "DELETE");
});
