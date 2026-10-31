// The strip of the open photo's folder under the editor, for moving between
// photos without going back to the Library.

import { invoke } from "@tauri-apps/api/core";
import { h } from "./panels";

interface Photo {
  path: string;
  file_name: string;
  edited: boolean;
}

interface Folder {
  path: string;
  name: string;
  photos: Photo[];
}

const KEY = "aperio.filmstrip";

export function buildFilmstrip(root: HTMLElement, open: (path: string) => void) {
  const title = h("strong", {});
  const count = h("span", { className: "muted" });
  const toggle = h("button", { type: "button", className: "link", title: "Show or hide the filmstrip (F)" });
  const thumbs = h("div", { className: "thumbs" });
  root.append(h("div", { className: "strip-head" }, title, count, h("span", { className: "spacer" }), toggle), thumbs);

  let folder: Folder | null = null;
  let current: string | null = null;
  let hidden = false;
  try {
    hidden = localStorage.getItem(KEY) === "hidden";
  } catch {
    // Shown, then.
  }
  const urls = new Map<string, string>();

  const show = () => {
    root.classList.toggle("collapsed", hidden);
    toggle.textContent = hidden ? "Show filmstrip" : "Hide";
  };
  const setHidden = (v: boolean) => {
    hidden = v;
    try {
      localStorage.setItem(KEY, v ? "hidden" : "shown");
    } catch {
      // Not remembered, then.
    }
    show();
  };
  toggle.addEventListener("click", () => setHidden(!hidden));
  show();

  // Fetch thumbnails as they scroll into view, one at a time.
  const queue: Array<() => Promise<void>> = [];
  let busy = false;
  const pump = async () => {
    if (busy) return;
    busy = true;
    while (queue.length) await queue.shift()!();
    busy = false;
  };
  const seen = new IntersectionObserver((entries) => {
    for (const e of entries) {
      if (!e.isIntersecting) continue;
      const el = e.target as HTMLElement;
      seen.unobserve(el);
      const path = el.dataset.path!;
      queue.push(async () => {
        let url = urls.get(path);
        if (!url) {
          try {
            const bytes = await invoke<ArrayBuffer | number[]>("thumbnail", { path });
            url = URL.createObjectURL(new Blob([new Uint8Array(bytes)], { type: "image/jpeg" }));
            urls.set(path, url);
          } catch {
            return;
          }
        }
        el.style.backgroundImage = `url(${url})`;
      });
      void pump();
    }
  }, { root: thumbs });

  const draw = () => {
    thumbs.replaceChildren();
    if (!folder) return;
    title.textContent = folder.name;
    const at = folder.photos.findIndex((p) => p.path === current);
    count.textContent = `${at >= 0 ? `${at + 1} of ` : ""}${folder.photos.length}`;
    for (const p of folder.photos) {
      const el = h("button", { type: "button", className: "thumb", title: p.file_name });
      el.dataset.path = p.path;
      if (p.path === current) el.classList.add("current");
      if (p.edited) el.append(h("span", { className: "edited", title: "Edited" }));
      const url = urls.get(p.path);
      if (url) el.style.backgroundImage = `url(${url})`;
      else seen.observe(el);
      el.addEventListener("click", () => p.path !== current && open(p.path));
      thumbs.append(el);
    }
    thumbs.querySelector(".current")?.scrollIntoView({ block: "nearest", inline: "center" });
  };

  return {
    /** Show `path`'s folder with `path` highlighted. */
    async show(path: string, folderPath: string) {
      current = path;
      if (folder?.path !== folderPath) {
        try {
          folder = await invoke<Folder>("list_folder", { path: folderPath });
        } catch {
          folder = null;
        }
      } else {
        // Its thumbnail now shows the latest edit.
        const old = urls.get(path);
        if (old) URL.revokeObjectURL(old);
        urls.delete(path);
      }
      draw();
    },
    /** The photo `step` places before or after the current one. */
    neighbor(step: number): string | null {
      if (!folder) return null;
      const i = folder.photos.findIndex((p) => p.path === current);
      return folder.photos[i + step]?.path ?? null;
    },
    toggle: () => setHidden(!hidden),
  };
}
