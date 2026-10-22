// Send to Claude Code: the island as a second prompt for a Claude Code session
// that runs in a terminal. What is typed (or dictated with Win+H) here is typed
// into that terminal and sent (session_input.rs); images pasted with Ctrl+V go
// along as file paths. The conversation shown is the session's own, as its
// hooks report it: every prompt (from the terminal or from here) and every
// final answer, so a message sent from here shows up once Claude Code took it.

import { h, svg, clear } from "./dom";
import { ICONS } from "./icons";
import { renderMarkdown } from "./markdown";
import { Bridge } from "../core/bridge";
import { Sound } from "../core/sound";
import { State } from "../core/state";
import type { ViewHost } from "./views";
import { N_, t, tl } from "../i18n/i18n";

const STRINGS = {
  placeholder: N_("Message Claude Code…"),
  noSession: N_("No Claude Code session yet. Talk to Claude Code once in its terminal and it shows up here."),
  sent: N_("Sent. Claude is working…"),
  answerFirst: N_("Answer Claude Code's request first."),
  failed: N_("Couldn't send: {error}"),
  imageFailed: N_("Couldn't add the image."),
  switchSession: N_("Switch session"),
  remove: N_("Remove"),
};

/** The hook fields this view reads. */
export interface SessionHook {
  hook_event_name?: string;
  session_id?: string;
  cwd?: string;
  prompt?: string;
  last_assistant_message?: string;
  coucou_agent?: string;
}

interface Turn {
  role: "user" | "assistant";
  text: string;
}

interface Session {
  id: string;
  project: string;
  turns: Turn[];
  seen: number;
}

/** The conversation kept per session; the oldest turns go first. */
const MAX_TURNS = 40;
const MAX_SESSIONS = 8;

const sessions = new Map<string, Session>();
let selected: string | null = null;
/** The session a message was sent to from here, until its answer comes. */
let awaiting: string | null = null;
let onChange: (() => void) | null = null;

function project(cwd: string): string {
  return cwd.split(/[\\/]/).filter(Boolean).pop() || "Claude Code";
}

/** Called by hooks.ts for every hook, after it has handled it. */
export function observeSessionHook(p: SessionHook) {
  // Claude Code's own sessions only: other agents are not typed into.
  if (p.coucou_agent || !p.session_id || p.hook_event_name === "StatusLine") return;
  const id = p.session_id;
  if (p.hook_event_name === "SessionEnd") {
    sessions.delete(id);
    if (selected === id) selected = null;
    if (awaiting === id) awaiting = null;
    onChange?.();
    return;
  }
  let s = sessions.get(id);
  if (!s) {
    s = { id, project: project(p.cwd ?? ""), turns: [], seen: 0 };
    sessions.set(id, s);
    if (sessions.size > MAX_SESSIONS) {
      const oldest = [...sessions.values()].sort((a, b) => a.seen - b.seen)[0];
      sessions.delete(oldest.id);
    }
  }
  s.seen = Date.now();
  // Background-task notices and the like reach Claude Code as prompts too:
  // what the person wrote is what is shown.
  if (p.hook_event_name === "UserPromptSubmit" && p.prompt && !/^\s*<[a-z-]+[\s>]/.test(p.prompt)) {
    s.turns.push({ role: "user", text: p.prompt });
  } else if (p.hook_event_name === "Stop") {
    if (p.last_assistant_message) s.turns.push({ role: "assistant", text: p.last_assistant_message });
    if (awaiting === id) awaiting = null;
  }
  s.turns.splice(0, Math.max(0, s.turns.length - MAX_TURNS));
  onChange?.();
}

/** A Stop the island should answer with this view rather than the Finished card. */
export function answersHere(sessionId: string | undefined): boolean {
  if (!sessionId || !sessions.has(sessionId)) return false;
  return awaiting === sessionId || (State.mode === "expanded" && State.view === "send");
}

/** The session messages go to: the one picked, else the latest one heard from. */
function target(): Session | null {
  if (selected && sessions.has(selected)) return sessions.get(selected)!;
  let latest: Session | null = null;
  for (const s of sessions.values()) if (!latest || s.seen > latest.seen) latest = s;
  return latest;
}

interface Attached {
  path: string;
  url: string;
}

function readSize(blob: Blob): Promise<{ w: number; h: number }> {
  return createImageBitmap(blob).then((b) => {
    const size = { w: b.width, h: b.height };
    b.close();
    return size;
  });
}

export function buildSend(hold: (on: boolean) => void): ViewHost {
  const dot = h("i", { class: "chip-dot" });
  const name = h("span");
  const chip = h("button", { class: "chip send-target", title: tl(STRINGS.switchSession) }, dot, name);
  const status = h("span", { class: "send-status" });
  const head = h("div", { class: "send-head" }, chip, status);
  const log = h("div", { class: "chat-log" });
  const thumbs = h("div", { class: "send-thumbs" });
  const input = h("input", {
    type: "text",
    class: "chat-input",
    placeholder: t(STRINGS.placeholder),
    spellcheck: "false",
  }) as HTMLInputElement;
  const send = h("button", { class: "send-btn", title: tl("Send") }, svg(ICONS.arrowUp, 11));
  const bar = h("div", { class: "chat-bar" }, input, send);
  const body = h("div", { class: "chat-body" }, head, log, thumbs, bar);
  const el = h("div", { class: "view" }, h("div", { class: "card wash chat-card send-card" }, body));
  (el.querySelector(".card") as HTMLElement).style.setProperty("--wash", "rgba(217,119,87,0.45)");

  let images: Attached[] = [];
  let sending = false;
  let note = "";
  let renderedKey = "";

  const say = (text: string) => {
    note = text;
    State.notify();
  };

  chip.addEventListener("click", () => {
    const list = [...sessions.values()].sort((a, b) => b.seen - a.seen);
    if (list.length < 2) return;
    const current = target();
    const i = list.findIndex((s) => s.id === current?.id);
    selected = list[(i + 1) % list.length].id;
    Sound.play("blip");
    State.notify();
  });

  function drawThumbs() {
    clear(thumbs);
    for (const img of images) {
      const x = h("button", { class: "send-thumb-x", title: tl(STRINGS.remove) }, svg(ICONS.xmark, 8));
      x.addEventListener("click", () => {
        URL.revokeObjectURL(img.url);
        images = images.filter((i) => i !== img);
        drawThumbs();
      });
      thumbs.append(h("div", { class: "send-thumb" }, h("img", { src: img.url, alt: "", draggable: "false" }), x));
    }
  }

  input.addEventListener("paste", (e) => {
    const items = [...(e.clipboardData?.items ?? [])].filter((i) => i.kind === "file" && i.type.startsWith("image/"));
    if (items.length === 0) return;
    e.preventDefault();
    for (const item of items) {
      const blob = item.getAsFile();
      if (!blob) continue;
      void (async () => {
        try {
          const { w, h: ht } = await readSize(blob);
          const bytes = Array.from(new Uint8Array(await blob.arrayBuffer()));
          const path = await Bridge.pasteImage(bytes, w, ht);
          images.push({ path, url: URL.createObjectURL(blob) });
          drawThumbs();
        } catch {
          say(t(STRINGS.imageFailed));
          Sound.play("error");
        }
      })();
    }
  });

  async function submit() {
    const text = input.value.trim();
    if ((!text && images.length === 0) || sending) return;
    const s = target();
    if (!s) {
      say(t(STRINGS.noSession));
      return;
    }
    // A permission card or a question waits in that terminal: keys typed now
    // would answer it.
    if (State.pendingApproval?.sessionId === s.id) {
      say(t(STRINGS.answerFirst));
      Sound.play("error");
      return;
    }
    sending = true;
    send.disabled = true;
    try {
      await Bridge.sendToSession(s.id, text, images.map((i) => i.path));
      input.value = "";
      for (const i of images) URL.revokeObjectURL(i.url);
      images = [];
      drawThumbs();
      awaiting = s.id;
      say(t(STRINGS.sent));
      Sound.play("send");
    } catch (err) {
      say(t(STRINGS.failed, { error: String(err).replace(/^Error:\s*/, "") }));
      Sound.play("error");
    } finally {
      sending = false;
      send.disabled = false;
      input.focus();
    }
  }

  send.addEventListener("click", () => void submit());
  input.addEventListener("keydown", (e) => {
    // Escape still folds the island; the other keys are the field's.
    if (e.key === "Escape") return;
    if (e.key === "Enter" && !e.isComposing) {
      e.preventDefault();
      void submit();
    }
    e.stopPropagation();
  });
  input.addEventListener("focus", () => hold(true));
  input.addEventListener("blur", () => hold(false));

  onChange = () => State.notify();

  return {
    el,
    sync() {
      const s = target();
      name.textContent = s ? s.project : "Claude Code";
      chip.classList.toggle("many", sessions.size > 1);
      // "Sent" lasts until the answer is in.
      if (s && awaiting !== s.id && note === t(STRINGS.sent)) note = "";
      status.textContent = s ? note : note || t(STRINGS.noSession);

      const waiting = !!s && awaiting === s.id;
      const key = `${s?.id ?? ""}:${s?.turns.length ?? 0}:${s?.turns.at(-1)?.text.length ?? 0}:${waiting}`;
      if (key !== renderedKey) {
        renderedKey = key;
        clear(log);
        for (const turn of s?.turns ?? []) {
          if (turn.role === "user") {
            log.append(h("div", { class: "chat-row user" }, h("div", { class: "bubble", text: turn.text })));
          } else {
            const reply = h("div", { class: "reply" });
            renderMarkdown(reply, turn.text);
            log.append(h("div", { class: "chat-row" }, reply));
          }
        }
        if (waiting) log.append(h("div", { class: "chat-row" }, h("div", { class: "typing" }, h("i"), h("i"), h("i"))));
        log.scrollTop = log.scrollHeight;
      }
    },
    focus() {
      input.focus();
    },
  };
}
