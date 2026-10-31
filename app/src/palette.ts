// The command palette (Ctrl/Cmd+K): every editor action, searchable by name,
// with its shortcut. The same list makes the shortcut sheet (?).

import { h } from "./panels";

export interface Command {
  title: string;
  group: string;
  /** Shown as typed, e.g. "Ctrl+Z" or "B". */
  keys?: string;
  run: () => void;
  enabled?: () => boolean;
}

const mac = /Mac|iPhone|iPad/.test(navigator.platform);
/** "Ctrl+Z" written the way this platform writes it. */
export const keyLabel = (keys: string) => (mac ? keys.replace(/Ctrl\+/g, "⌘").replace(/Shift\+/g, "⇧").replace(/Alt\+/g, "⌥") : keys);

/** Score how well `query` matches `text`: every query character in order,
 * with a bonus for word starts and runs. Null means no match. */
function score(query: string, text: string): number | null {
  const q = query.toLowerCase();
  const t = text.toLowerCase();
  if (!q) return 0;
  let s = 0;
  let at = 0;
  let run = 0;
  for (const c of q) {
    const i = t.indexOf(c, at);
    if (i < 0) return null;
    run = i === at ? run + 1 : 0;
    s += 1 + run * 2 + (i === 0 || t[i - 1] === " " ? 3 : 0) - (i - at) * 0.05;
    at = i + 1;
  }
  return s;
}

export function buildPalette(commands: () => Command[]) {
  const input = h("input", { type: "text", placeholder: "Type a command…", spellcheck: false });
  const list = h("ul", { className: "palette-list" });
  const dialog = h("dialog", { className: "palette" }, h("div", { className: "palette-input" }, input, h("kbd", {}, "esc")), list);
  document.body.append(dialog);

  let shown: Command[] = [];
  let active = 0;

  const draw = () => {
    const q = input.value.trim();
    shown = commands()
      .filter((c) => c.enabled?.() ?? true)
      .map((c) => ({ c, s: score(q, `${c.title} ${c.group}`) }))
      .filter((x) => x.s !== null)
      .sort((a, b) => b.s! - a.s!)
      .map((x) => x.c);
    if (!q) shown.sort((a, b) => a.group.localeCompare(b.group));
    active = Math.min(active, Math.max(0, shown.length - 1));
    list.replaceChildren();
    let group = "";
    shown.forEach((c, i) => {
      if (!q && c.group !== group) {
        group = c.group;
        list.append(h("li", { className: "palette-group" }, group));
      }
      const row = h("li", { className: i === active ? "active" : "" }, h("span", {}, c.title), h("span", { className: "spacer" }), h("span", { className: "muted" }, q ? c.group : ""));
      if (c.keys) row.append(h("kbd", {}, keyLabel(c.keys)));
      row.addEventListener("mousemove", () => {
        if (active !== i) {
          active = i;
          draw();
        }
      });
      row.addEventListener("click", () => run(c));
      list.append(row);
    });
    if (shown.length === 0) list.append(h("li", { className: "palette-empty muted" }, "No matching command"));
    list.querySelector("li.active")?.scrollIntoView({ block: "nearest" });
  };

  const run = (c: Command) => {
    dialog.close();
    c.run();
  };

  input.addEventListener("input", () => {
    active = 0;
    draw();
  });
  input.addEventListener("keydown", (e) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      active = (active + (e.key === "ArrowDown" ? 1 : -1) + shown.length) % Math.max(1, shown.length);
      draw();
    } else if (e.key === "Enter") {
      e.preventDefault();
      if (shown[active]) run(shown[active]);
    }
  });
  dialog.addEventListener("click", (e) => e.target === dialog && dialog.close());

  // The shortcut sheet.
  const sheet = h("dialog", { className: "shortcuts" });
  document.body.append(sheet);
  sheet.addEventListener("click", () => sheet.close());

  return {
    open() {
      input.value = "";
      active = 0;
      draw();
      dialog.showModal();
      input.focus();
    },
    shortcuts() {
      const groups = new Map<string, Command[]>();
      for (const c of commands()) if (c.keys) groups.set(c.group, [...(groups.get(c.group) ?? []), c]);
      sheet.replaceChildren(
        h("header", {}, h("h2", {}, "Keyboard shortcuts"), h("span", { className: "muted" }, "Every action is also in the command palette, ", h("kbd", {}, keyLabel("Ctrl+K")))),
        h(
          "div",
          { className: "shortcut-grid" },
          ...[...groups].map(([g, cs]) => h("section", {}, h("h3", {}, g), ...cs.map((c) => h("div", { className: "shortcut" }, h("span", {}, c.title), h("kbd", {}, keyLabel(c.keys!)))))),
        ),
      );
      sheet.showModal();
    },
  };
}
