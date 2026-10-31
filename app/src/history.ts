// The History list in the left panel: every recorded change to this photo,
// newest first. Clicking a step shows the photo as it was then; later steps
// stay available until a new change replaces them.

import { h } from "./panels";
import { editor, goTo, history, onUpdate } from "./state";

export function buildHistory(root: HTMLElement) {
  const count = h("span", { className: "muted" });
  const list = h("ol", { className: "history-list" });
  root.append(h("div", { className: "group-head" }, h("h3", {}, "History"), count), list);
  let last = "";
  onUpdate(() => {
    const { labels, current } = editor.opened ? history() : { labels: [], current: 0 };
    const key = `${labels.join("\n")}#${current}`;
    if (key === last) return;
    last = key;
    count.textContent = labels.length > 1 ? `${labels.length - 1} change${labels.length === 2 ? "" : "s"}` : "";
    list.replaceChildren(
      ...labels
        .map((label, i) => {
          const row = h("li", { className: i === current ? "current" : i > current ? "undone" : "" }, h("span", {}, label));
          row.addEventListener("click", () => goTo(i));
          return row;
        })
        .reverse(),
    );
  });
}
