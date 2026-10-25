// The Library screen: browse the photos in a folder, in place. There is no
// catalog; the folder is read each time and edits stay in sidecars.

import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { h } from "./panels";

interface Photo {
  path: string;
  file_name: string;
  kind: "raw" | "jpeg" | "png" | "tiff";
  size_bytes: number;
  modified_ms: number;
  edited: boolean;
}

interface Folder {
  path: string;
  name: string;
  photos: Photo[];
}

interface Details {
  width: number | null;
  height: number | null;
  camera: string | null;
  lens: string | null;
  exposure: string | null;
  captured: string | null;
}

interface Recent {
  path: string;
  name: string;
  count: number;
}

interface Prefs {
  recent: Recent[];
  sort: "name" | "modified";
  size: number;
  edited: boolean;
  rawOnly: boolean;
}

export interface LibraryActions {
  /** Open a photo in the editor. */
  edit(path: string): Promise<void>;
  /** Open a photo in the editor, then its export screen. */
  quickExport(path: string, thumbnail: string | undefined): Promise<void>;
}

const STORE = "aperio.library";
const MAX_RECENT = 8;
const KIND_LABEL: Record<Photo["kind"], string> = { raw: "RAW", jpeg: "JPEG", png: "PNG", tiff: "TIFF" };
/** Thumbnails decoded at once. RAW previews are quick, but not free. */
const PARALLEL = 3;

function loadPrefs(): Prefs {
  const defaults: Prefs = { recent: [], sort: "name", size: 160, edited: false, rawOnly: false };
  try {
    return { ...defaults, ...(JSON.parse(localStorage.getItem(STORE) ?? "{}") as Partial<Prefs>) };
  } catch {
    return defaults;
  }
}

function megabytes(bytes: number) {
  return bytes >= 1e6 ? `${(bytes / 1e6).toFixed(1)} MB` : `${Math.max(1, Math.round(bytes / 1e3))} KB`;
}

/** EXIF dates look like "2026:10:12 07:41:03". */
function captureLabel(s: string) {
  const m = /^(\d{4}):(\d{2}):(\d{2})[ T](\d{2}):(\d{2})/.exec(s);
  if (!m) return s;
  return new Date(+m[1], +m[2] - 1, +m[3], +m[4], +m[5]).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" });
}

export function buildLibrary(root: HTMLElement, search: HTMLInputElement, actions: LibraryActions) {
  let prefs = loadPrefs();
  let folder: Folder | null = null;
  let selected: string | null = null;
  let detailsFor = "";

  const savePrefs = () => {
    try {
      localStorage.setItem(STORE, JSON.stringify(prefs));
    } catch {
      // Not remembered, then.
    }
  };

  // --- Thumbnails: fetched as tiles scroll into view, a few at a time. ---

  const urls = new Map<string, string>();
  const waiting: Array<() => Promise<void>> = [];
  let running = 0;
  const pump = () => {
    while (running < PARALLEL && waiting.length > 0) {
      running++;
      void waiting.shift()!().finally(() => {
        running--;
        pump();
      });
    }
  };
  function thumbnail(path: string, show: (url: string) => void, failed: () => void) {
    const have = urls.get(path);
    if (have) return show(have);
    waiting.push(async () => {
      try {
        const bytes = await invoke<ArrayBuffer | number[]>("thumbnail", { path });
        const url = URL.createObjectURL(new Blob([new Uint8Array(bytes)], { type: "image/jpeg" }));
        urls.set(path, url);
        show(url);
      } catch {
        failed();
      }
    });
    pump();
  }
  function forget(path: string) {
    const url = urls.get(path);
    if (url) URL.revokeObjectURL(url);
    urls.delete(path);
  }

  // --- Layout ---

  const recentList = h("ul", { className: "folders" });
  const editedBox = h("input", { type: "checkbox" });
  const rawBox = h("input", { type: "checkbox" });
  editedBox.addEventListener("change", () => setPref({ edited: editedBox.checked }));
  rawBox.addEventListener("change", () => setPref({ rawOnly: rawBox.checked }));
  const side = h(
    "nav",
    { className: "lib-side" },
    h("h4", {}, "Recent folders"),
    recentList,
    h("h4", {}, "Filter"),
    h("label", { className: "check" }, editedBox, "Edited"),
    h("label", { className: "check" }, rawBox, "RAW only"),
    h("p", { className: "note muted" }, "Aperio reads folders in place. Edits live in sidecar files beside each photo, so there is no catalog to migrate."),
  );

  const heading = h("h2", {});
  const counts = h("span", { className: "muted" });
  const sort = h("select", { id: "lib-sort" });
  sort.append(h("option", { value: "name", textContent: "Name" }), h("option", { value: "modified", textContent: "Date modified" }));
  sort.addEventListener("change", () => setPref({ sort: sort.value as Prefs["sort"] }));
  const size = h("input", { type: "range", min: "100", max: "340", step: "10", id: "lib-size" });
  size.addEventListener("input", () => setPref({ size: Number(size.value) }));
  const bar = h(
    "div",
    { className: "lib-bar" },
    heading,
    counts,
    h("span", { className: "spacer" }),
    h("label", { className: "muted", htmlFor: "lib-sort" }, "Sort"),
    sort,
    h("label", { className: "muted", htmlFor: "lib-size" }, "Size"),
    size,
  );
  const grid = h("div", { className: "lib-grid", role: "listbox" });
  grid.setAttribute("aria-label", "Photos");
  const empty = h("div", { className: "lib-empty" });
  const main = h("section", { className: "lib-main" }, bar, grid, empty);

  const inspector = h("aside", { className: "lib-inspector" });

  root.append(side, main, inspector);

  const observer = new IntersectionObserver(
    (entries) => {
      for (const e of entries) {
        if (!e.isIntersecting) continue;
        observer.unobserve(e.target);
        const tile = e.target as HTMLButtonElement;
        const img = tile.querySelector("img")!;
        thumbnail(
          tile.dataset.path!,
          (url) => (img.src = url),
          () => tile.classList.add("broken"),
        );
      }
    },
    { root: grid, rootMargin: "300px" },
  );

  // --- Rendering ---

  function visible(): Photo[] {
    if (!folder) return [];
    const q = search.value.trim().toLowerCase();
    const list = folder.photos.filter((p) => (!prefs.edited || p.edited) && (!prefs.rawOnly || p.kind === "raw") && (!q || p.file_name.toLowerCase().includes(q)));
    if (prefs.sort === "modified") list.sort((a, b) => b.modified_ms - a.modified_ms);
    return list;
  }

  function drawRecent() {
    recentList.replaceChildren(
      ...prefs.recent.map((r) => {
        const b = h("button", { type: "button", title: r.path }, h("span", { className: "name" }, r.name), h("span", { className: "count mono" }, String(r.count)));
        b.classList.toggle("active", r.path === folder?.path);
        b.addEventListener("click", () => void openFolder(r.path));
        return h("li", {}, b);
      }),
    );
    if (prefs.recent.length === 0) recentList.append(h("li", { className: "muted none" }, "None yet"));
  }

  function drawGrid() {
    observer.disconnect();
    grid.style.setProperty("--tile", `${prefs.size}px`);
    const photos = visible();
    grid.replaceChildren(
      ...photos.map((p) => {
        const img = h("img", { alt: "", draggable: false });
        const tile = h("button", { type: "button", className: "tile", title: p.file_name }, img, h("span", { className: "badge kind mono" }, KIND_LABEL[p.kind]));
        tile.setAttribute("role", "option");
        tile.setAttribute("aria-label", `${p.file_name}${p.edited ? ", edited" : ""}`);
        if (p.edited) tile.append(h("span", { className: "badge edited" }, "Edited"));
        tile.dataset.path = p.path;
        tile.classList.toggle("selected", p.path === selected);
        tile.setAttribute("aria-selected", String(p.path === selected));
        tile.addEventListener("click", () => select(p.path));
        tile.addEventListener("dblclick", () => void actions.edit(p.path));
        observer.observe(tile);
        return tile;
      }),
    );

    if (!folder) {
      heading.textContent = "Library";
      counts.textContent = "";
    } else {
      heading.textContent = folder.name;
      const raw = folder.photos.filter((p) => p.kind === "raw").length;
      const other = folder.photos.length - raw;
      const parts = [`${folder.photos.length} photo${folder.photos.length === 1 ? "" : "s"}`];
      if (raw && other) parts.push(`${raw} RAW`, `${other} other`);
      if (photos.length !== folder.photos.length) parts.push(`showing ${photos.length}`);
      counts.textContent = parts.join(" · ");
    }
    bar.hidden = !folder;
    empty.hidden = !!folder && photos.length > 0;
    const choose = h("button", { type: "button", className: "primary", textContent: "Open folder…" });
    choose.addEventListener("click", () => void chooseFolder());
    if (!folder) empty.replaceChildren(h("p", {}, "Open a folder to browse its photos."), choose);
    else if (folder.photos.length === 0) empty.replaceChildren(h("p", {}, "There are no photos Aperio can open in this folder."), choose);
    else empty.replaceChildren(h("p", { className: "muted" }, "No photos match the search or filters."));
  }

  function drawInspector() {
    const p = folder?.photos.find((x) => x.path === selected);
    inspector.hidden = !p;
    if (!p) return;
    const img = h("img", { className: "thumb", alt: "" });
    thumbnail(p.path, (url) => (img.src = url), () => {});
    const facts = h("dl", { className: "facts" });
    const row = (label: string, value: string, mono = false) => facts.append(h("dt", {}, label), h("dd", { className: mono ? "mono" : "" }, value));
    row("Type", KIND_LABEL[p.kind]);
    row("File size", megabytes(p.size_bytes), true);
    row("Modified", new Date(p.modified_ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }));
    row("Sidecar", p.edited ? `${p.file_name}.aperio.json` : "None yet");
    const extra = h("dl", { className: "facts" });
    const openIt = h("button", { type: "button", className: "primary", textContent: "Open in editor" });
    openIt.addEventListener("click", () => void actions.edit(p.path));
    const quick = h("button", { type: "button", textContent: "Quick export…" });
    quick.addEventListener("click", () => void actions.quickExport(p.path, urls.get(p.path)));
    inspector.replaceChildren(img, h("h3", {}, p.file_name), extra, facts, h("span", { className: "spacer" }), openIt, quick);

    // Camera details come from the file, so fetch them after drawing.
    detailsFor = p.path;
    void invoke<Details>("photo_details", { path: p.path })
      .then((d) => {
        if (detailsFor !== p.path) return;
        const add = (label: string, value: string | null, mono = false) => {
          if (value) extra.append(h("dt", {}, label), h("dd", { className: mono ? "mono" : "" }, value));
        };
        add("Camera", d.camera);
        add("Lens", d.lens);
        add("Exposure", d.exposure, true);
        add("Captured", d.captured && captureLabel(d.captured));
        add("Size", d.width && d.height ? `${d.width} × ${d.height}` : null, true);
      })
      .catch(() => {});
  }

  function draw() {
    sort.value = prefs.sort;
    size.value = String(prefs.size);
    editedBox.checked = prefs.edited;
    rawBox.checked = prefs.rawOnly;
    drawRecent();
    drawGrid();
    drawInspector();
  }

  function setPref(change: Partial<Prefs>) {
    prefs = { ...prefs, ...change };
    savePrefs();
    draw();
  }

  function select(path: string | null) {
    selected = path;
    for (const t of grid.querySelectorAll<HTMLButtonElement>(".tile")) {
      const on = t.dataset.path === path;
      t.classList.toggle("selected", on);
      t.setAttribute("aria-selected", String(on));
      if (on) t.focus({ preventScroll: true });
    }
    grid.querySelector(".tile.selected")?.scrollIntoView({ block: "nearest" });
    drawInspector();
  }

  async function openFolder(path: string, quiet = false) {
    try {
      const next = await invoke<Folder>("list_folder", { path });
      if (next.path !== folder?.path) {
        for (const url of urls.values()) URL.revokeObjectURL(url);
        urls.clear();
        waiting.length = 0;
        selected = next.photos[0]?.path ?? null;
      } else if (!next.photos.some((p) => p.path === selected)) {
        selected = next.photos[0]?.path ?? null;
      }
      folder = next;
      const recent = { path: next.path, name: next.name, count: next.photos.length };
      setPref({ recent: [recent, ...prefs.recent.filter((r) => r.path !== next.path)].slice(0, MAX_RECENT) });
    } catch (e) {
      // A folder that was moved or deleted drops off the recent list.
      setPref({ recent: prefs.recent.filter((r) => r.path !== path) });
      if (!quiet) alert(`Could not open the folder: ${e}`);
      else draw();
    }
  }

  async function chooseFolder() {
    const picked = await open({ directory: true, multiple: false, defaultPath: folder?.path });
    if (typeof picked === "string") await openFolder(picked);
  }

  search.addEventListener("input", drawGrid);

  /** Arrow keys move through the grid; Enter opens the photo. */
  function key(e: KeyboardEvent) {
    const photos = visible();
    const i = photos.findIndex((p) => p.path === selected);
    const columns = Math.max(1, getComputedStyle(grid).gridTemplateColumns.split(" ").length);
    const step: Record<string, number> = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -columns, ArrowDown: columns };
    if (e.key in step && photos.length > 0) {
      e.preventDefault();
      const next = Math.min(photos.length - 1, Math.max(0, (i < 0 ? 0 : i) + (i < 0 ? 0 : step[e.key])));
      select(photos[next].path);
    } else if (e.key === "Enter" && selected) {
      e.preventDefault();
      void actions.edit(selected);
    }
  }

  draw();

  return {
    /** Show the library, re-reading the folder so badges are current. */
    async show(edited?: string) {
      if (edited) forget(edited);
      if (folder) await openFolder(folder.path);
      else if (prefs.recent[0]) await openFolder(prefs.recent[0].path, true);
      else draw();
    },
    openFolder,
    chooseFolder,
    key,
  };
}
