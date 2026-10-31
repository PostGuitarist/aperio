// The Library screen: browse the photos in a folder, in place, or in an
// album or smart collection. Folders are read each time; edits, ratings,
// flags, and labels stay in sidecars beside each photo. Albums and smart
// collections are kept in one small file in the config folder.

import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import { h } from "./panels";

export type Flag = "pick" | "reject";
export type Label = "red" | "yellow" | "green" | "blue" | "purple";

export interface Photo {
  path: string;
  file_name: string;
  kind: "raw" | "jpeg" | "png" | "tiff" | "heif";
  size_bytes: number;
  modified_ms: number;
  edited: boolean;
  rating: number;
  flag: Flag | null;
  label: Label | null;
}

interface Album {
  name: string;
  photos: string[];
}

interface Smart {
  name: string;
  min_rating: number;
  flag?: Flag;
  label?: Label;
  edited?: boolean;
  kind?: Photo["kind"];
  name_contains?: string;
}

interface Collections {
  albums: Album[];
  smart: Smart[];
}

/** What the grid shows. */
type Source = { kind: "folder"; path: string } | { kind: "album"; name: string } | { kind: "smart"; name: string };

type SyncPart = "light" | "color" | "effects" | "curves" | "detail" | "crop" | "layers";
const SYNC_PARTS: Array<[SyncPart, string]> = [
  ["light", "Light"],
  ["color", "Color"],
  ["effects", "Texture, clarity, dehaze"],
  ["curves", "Tone curves"],
  ["detail", "Denoise and plugins"],
  ["crop", "Crop"],
  ["layers", "Masked layers"],
];

export const LABELS: Label[] = ["red", "yellow", "green", "blue", "purple"];
const LABEL_KEYS: Record<string, Label> = { "6": "red", "7": "yellow", "8": "green", "9": "blue" };

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
  sort: "name" | "modified" | "rating";
  size: number;
  edited: boolean;
  rawOnly: boolean;
  minRating: number;
  flag: "any" | "pick" | "unflagged" | "reject";
  label: Label | "any";
  sync: SyncPart[];
}

export interface LibraryActions {
  /** Open a photo in the editor. */
  edit(path: string): Promise<void>;
  /** Open a photo in the editor, then its export screen. */
  quickExport(path: string, thumbnail: string | undefined): Promise<void>;
  /** Edits of these photos were changed by syncing. */
  synced?(paths: string[]): void | Promise<void>;
}

const STORE = "aperio.library";
const MAX_RECENT = 8;
const KIND_LABEL: Record<Photo["kind"], string> = { raw: "RAW", jpeg: "JPEG", png: "PNG", tiff: "TIFF", heif: "HEIC" };
/** Thumbnails decoded at once. RAW previews are quick, but not free. */
const PARALLEL = 3;

function loadPrefs(): Prefs {
  const defaults: Prefs = { recent: [], sort: "name", size: 160, edited: false, rawOnly: false, minRating: 0, flag: "any", label: "any", sync: ["light", "color", "effects", "curves"] };
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

function stars(n: number) {
  return "★".repeat(n) + "☆".repeat(5 - n);
}

export function buildLibrary(root: HTMLElement, search: HTMLInputElement, actions: LibraryActions) {
  let prefs = loadPrefs();
  let source: Source | null = null;
  /** The shown source's title and photos. */
  let title = "";
  let photos: Photo[] = [];
  /** Photos picked in the grid; `selected` is the one the inspector shows,
   * and the one edits are synced from. */
  let picked = new Set<string>();
  let selected: string | null = null;
  /** Where Shift-click selects from. */
  let anchor: string | null = null;
  let collections: Collections = { albums: [], smart: [] };
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
  const albumList = h("ul", { className: "folders" });
  const smartList = h("ul", { className: "folders" });
  const newAlbum = h("button", { type: "button", className: "icon", title: "New album", textContent: "+" });
  const newSmart = h("button", { type: "button", className: "icon", title: "New smart collection", textContent: "+" });
  const albumForm = h("form", { className: "inline-name", hidden: true });
  const albumName = h("input", { type: "text", placeholder: "Album name", maxLength: 60 });
  albumForm.append(albumName, h("button", { type: "submit", textContent: "Create" }));
  const smartForm = h("form", { className: "smart-form", hidden: true });

  const editedBox = h("input", { type: "checkbox" });
  const rawBox = h("input", { type: "checkbox" });
  editedBox.addEventListener("change", () => setPref({ edited: editedBox.checked }));
  rawBox.addEventListener("change", () => setPref({ rawOnly: rawBox.checked }));
  const ratingFilter = h("div", { className: "stars filter", title: "Show photos rated at least this" });
  for (let n = 1; n <= 5; n++) {
    const b = h("button", { type: "button", textContent: "★", title: `${n} star${n === 1 ? "" : "s"} or more` });
    b.addEventListener("click", () => setPref({ minRating: prefs.minRating === n ? 0 : n }));
    ratingFilter.append(b);
  }
  const flagFilter = h("div", { className: "segmented small" });
  for (const [value, text] of [
    ["any", "All"],
    ["pick", "Picks"],
    ["unflagged", "Unflagged"],
    ["reject", "Rejects"],
  ] as Array<[Prefs["flag"], string]>) {
    const b = h("button", { type: "button", textContent: text });
    b.dataset.value = value;
    b.addEventListener("click", () => setPref({ flag: value }));
    flagFilter.append(b);
  }
  const labelFilter = h("div", { className: "labels filter" });
  for (const l of LABELS) {
    const b = h("button", { type: "button", className: `dot ${l}`, title: `${l[0].toUpperCase()}${l.slice(1)} label` });
    b.dataset.value = l;
    b.addEventListener("click", () => setPref({ label: prefs.label === l ? "any" : l }));
    labelFilter.append(b);
  }

  const side = h(
    "nav",
    { className: "lib-side" },
    h("h4", {}, "Recent folders"),
    recentList,
    h("h4", { className: "with-add" }, h("span", {}, "Albums"), newAlbum),
    albumForm,
    albumList,
    h("h4", { className: "with-add" }, h("span", {}, "Smart collections"), newSmart),
    smartForm,
    smartList,
    h("h4", {}, "Filter"),
    ratingFilter,
    flagFilter,
    labelFilter,
    h("label", { className: "check" }, editedBox, "Edited"),
    h("label", { className: "check" }, rawBox, "RAW only"),
    h("p", { className: "note muted" }, "Edits, ratings, flags, and labels live in sidecar files beside each photo, so they move with it. Albums and smart collections are kept in Aperio's settings."),
  );

  const heading = h("h2", {});
  const counts = h("span", { className: "muted" });
  const sort = h("select", { id: "lib-sort" });
  sort.append(h("option", { value: "name", textContent: "Name" }), h("option", { value: "modified", textContent: "Date modified" }), h("option", { value: "rating", textContent: "Rating" }));
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
  grid.setAttribute("aria-multiselectable", "true");
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

  // --- Collections ---

  const saveCollections = async () => {
    try {
      await invoke("save_collections", { collections });
    } catch (e) {
      alert(`Could not save albums: ${e}`);
    }
  };

  newAlbum.addEventListener("click", () => {
    albumForm.hidden = !albumForm.hidden;
    if (!albumForm.hidden) {
      albumName.value = "";
      albumName.focus();
    }
  });
  albumForm.addEventListener("submit", (e) => {
    e.preventDefault();
    const name = albumName.value.trim();
    if (!name || collections.albums.some((a) => a.name === name)) return;
    // A new album starts with the photos picked in the grid, if any.
    collections.albums.push({ name, photos: [...picked] });
    albumForm.hidden = true;
    void saveCollections().then(() => openSource({ kind: "album", name }));
  });

  // The smart collection form: a name and the rules to match.
  const smartName = h("input", { type: "text", placeholder: "Collection name", maxLength: 60 });
  const select = (options: Array<[string, string]>) => {
    const el = h("select", {}) as HTMLSelectElement;
    for (const [value, text] of options) el.append(h("option", { value, textContent: text }));
    return el;
  };
  const smartRating = select([["0", "Any rating"], ...[1, 2, 3, 4, 5].map((n) => [String(n), `${stars(n).slice(0, n)} or more`] as [string, string])]);
  const smartFlag = select([["", "Any flag"], ["pick", "Picks"], ["reject", "Rejects"]]);
  const smartLabel = select([["", "Any label"], ...LABELS.map((l) => [l, `${l[0].toUpperCase()}${l.slice(1)} label`] as [string, string])]);
  const smartEdited = select([["", "Edited or not"], ["true", "Edited"], ["false", "Not edited"]]);
  const smartKind = select([["", "Any file type"], ...Object.entries(KIND_LABEL)]);
  smartForm.append(smartName, smartRating, smartFlag, smartLabel, smartEdited, smartKind, h("button", { type: "submit", textContent: "Create" }));
  newSmart.addEventListener("click", () => {
    smartForm.hidden = !smartForm.hidden;
    if (!smartForm.hidden) {
      smartName.value = "";
      smartName.focus();
    }
  });
  smartForm.addEventListener("submit", (e) => {
    e.preventDefault();
    const name = smartName.value.trim();
    if (!name || collections.smart.some((c) => c.name === name)) return;
    const rule: Smart = { name, min_rating: Number(smartRating.value) };
    if (smartFlag.value) rule.flag = smartFlag.value as Flag;
    if (smartLabel.value) rule.label = smartLabel.value as Label;
    if (smartEdited.value) rule.edited = smartEdited.value === "true";
    if (smartKind.value) rule.kind = smartKind.value as Photo["kind"];
    collections.smart.push(rule);
    smartForm.hidden = true;
    void saveCollections().then(() => openSource({ kind: "smart", name }));
  });
  for (const input of [albumName, smartName]) input.addEventListener("keydown", (e) => e.stopPropagation());

  function matches(rule: Smart, p: Photo) {
    return (
      p.rating >= rule.min_rating &&
      (!rule.flag || p.flag === rule.flag) &&
      (!rule.label || p.label === rule.label) &&
      (rule.edited === undefined || p.edited === rule.edited) &&
      (!rule.kind || p.kind === rule.kind) &&
      (!rule.name_contains || p.file_name.toLowerCase().includes(rule.name_contains.toLowerCase()))
    );
  }

  /** Read whatever `src` names: a folder, an album's photos, or every
   * recent folder for a smart collection. */
  async function read(src: Source): Promise<{ title: string; photos: Photo[] }> {
    if (src.kind === "folder") {
      const f = await invoke<Folder>("list_folder", { path: src.path });
      const recent = { path: f.path, name: f.name, count: f.photos.length };
      prefs.recent = [recent, ...prefs.recent.filter((r) => r.path !== f.path)].slice(0, MAX_RECENT);
      savePrefs();
      return { title: f.name, photos: f.photos };
    }
    if (src.kind === "album") {
      const album = collections.albums.find((a) => a.name === src.name);
      if (!album) throw new Error("This album no longer exists.");
      return { title: album.name, photos: await invoke<Photo[]>("list_photos", { paths: album.photos }) };
    }
    const rule = collections.smart.find((c) => c.name === src.name);
    if (!rule) throw new Error("This collection no longer exists.");
    const folders = await Promise.all(prefs.recent.map((r) => invoke<Folder>("list_folder", { path: r.path }).catch(() => null)));
    const all = folders.flatMap((f) => f?.photos ?? []);
    return { title: rule.name, photos: all.filter((p) => matches(rule, p)) };
  }

  const sameSource = (a: Source | null, b: Source | null) => JSON.stringify(a) === JSON.stringify(b);

  async function openSource(src: Source, quiet = false) {
    try {
      const next = await read(src);
      if (!sameSource(src, source)) {
        picked = new Set();
        anchor = null;
        selected = next.photos[0]?.path ?? null;
      } else if (!next.photos.some((p) => p.path === selected)) {
        selected = next.photos[0]?.path ?? null;
      }
      picked = new Set([...picked].filter((p) => next.photos.some((x) => x.path === p)));
      if (selected) picked.add(selected);
      source = src;
      title = next.title;
      photos = next.photos;
      draw();
    } catch (e) {
      if (src.kind === "folder") {
        // A folder that was moved or deleted drops off the recent list.
        setPref({ recent: prefs.recent.filter((r) => r.path !== src.path) });
        if (!quiet) alert(`Could not open the folder: ${e}`);
      } else if (!quiet) alert(String(e));
      draw();
    }
  }

  // --- Marks ---

  async function mark(change: { rating?: number; flag?: Flag | "none"; label?: Label | "none" }, paths = [...picked]) {
    if (paths.length === 0) return;
    try {
      const updated = await invoke<Photo[]>("set_marks", { paths, change });
      for (const u of updated) {
        const i = photos.findIndex((p) => p.path === u.path);
        if (i >= 0) photos[i] = u;
      }
      drawGrid();
      drawInspector();
    } catch (e) {
      alert(`Could not save: ${e}`);
    }
  }

  /** Star, flag, and label buttons for the picked photos. */
  function markControls(list: Photo[]) {
    const all = <T>(f: (p: Photo) => T) => (list.every((p) => f(p) === f(list[0])) ? f(list[0]) : undefined);
    const rating = all((p) => p.rating);
    const flag = all((p) => p.flag);
    const label = all((p) => p.label);

    const starRow = h("div", { className: "stars" });
    for (let n = 1; n <= 5; n++) {
      const b = h("button", { type: "button", textContent: "★", title: `${n} star${n === 1 ? "" : "s"} (${n})` });
      b.classList.toggle("on", rating !== undefined && n <= rating);
      b.addEventListener("click", () => void mark({ rating: rating === n ? 0 : n }));
      starRow.append(b);
    }
    const flags = h("div", { className: "segmented small" });
    for (const [value, text, key] of [
      ["pick", "⚑ Pick", "P"],
      ["none", "Unflag", "U"],
      ["reject", "✕ Reject", "X"],
    ] as Array<[Flag | "none", string, string]>) {
      const b = h("button", { type: "button", textContent: text, title: `${text.replace(/^\W+ /, "")} (${key})` });
      b.classList.toggle("active", flag !== undefined && (flag ?? "none") === value);
      b.addEventListener("click", () => void mark({ flag: value }));
      flags.append(b);
    }
    const labels = h("div", { className: "labels" });
    for (const l of LABELS) {
      const key = Object.entries(LABEL_KEYS).find(([, v]) => v === l)?.[0];
      const b = h("button", { type: "button", className: `dot ${l}`, title: `${l[0].toUpperCase()}${l.slice(1)}${key ? ` (${key})` : ""}` });
      b.classList.toggle("on", label === l);
      b.addEventListener("click", () => void mark({ label: label === l ? "none" : l }));
      labels.append(b);
    }
    return h("div", { className: "marks" }, starRow, flags, labels);
  }

  // --- Albums from the inspector ---

  function albumControls(paths: string[]) {
    const wrap = h("div", { className: "album-tools" });
    const choose = h("select", {}) as HTMLSelectElement;
    choose.append(h("option", { value: "", textContent: "Add to album…" }));
    for (const a of collections.albums) choose.append(h("option", { value: a.name, textContent: a.name }));
    choose.append(h("option", { value: "+", textContent: "New album…" }));
    choose.addEventListener("change", () => {
      if (choose.value === "+") {
        albumForm.hidden = false;
        albumName.value = "";
        albumName.focus();
      } else {
        const album = collections.albums.find((a) => a.name === choose.value);
        if (album) {
          album.photos = [...new Set([...album.photos, ...paths])];
          void saveCollections().then(() => drawSide());
        }
      }
      choose.value = "";
    });
    wrap.append(choose);
    if (source?.kind === "album") {
      const name = source.name;
      const remove = h("button", { type: "button", textContent: `Remove from ${name}` });
      remove.addEventListener("click", () => {
        const album = collections.albums.find((a) => a.name === name);
        if (!album) return;
        album.photos = album.photos.filter((p) => !paths.includes(p));
        void saveCollections().then(() => openSource({ kind: "album", name }));
      });
      wrap.append(remove);
    }
    return wrap;
  }

  // --- Syncing edits ---

  function syncControls(from: Photo, to: string[]) {
    const box = h("div", { className: "sync" });
    const list = h("div", { className: "sync-parts" });
    for (const [part, text] of SYNC_PARTS) {
      const check = h("input", { type: "checkbox" });
      check.checked = prefs.sync.includes(part);
      check.addEventListener("change", () => {
        prefs.sync = check.checked ? [...prefs.sync, part] : prefs.sync.filter((p) => p !== part);
        savePrefs();
      });
      list.append(h("label", { className: "check" }, check, text));
    }
    const go = h("button", { type: "button", className: "primary", textContent: `Sync to ${to.length} photo${to.length === 1 ? "" : "s"}` });
    go.addEventListener("click", async () => {
      if (prefs.sync.length === 0) return;
      go.disabled = true;
      go.textContent = "Syncing…";
      try {
        const n = await invoke<number>("sync_edits", { from: from.path, to, parts: prefs.sync });
        for (const p of to) forget(p);
        await actions.synced?.(to);
        if (source) await openSource(source);
        status(n === 0 ? "Nothing to change: those photos already match." : `Synced ${n} photo${n === 1 ? "" : "s"}.`);
      } catch (e) {
        alert(`Could not sync: ${e}`);
        drawInspector();
      }
    });
    box.append(h("h4", {}, "Sync edits"), h("p", { className: "muted" }, `From ${from.file_name}, the photo shown above, to the other selected photos:`), list, go);
    return box;
  }

  let notice = "";
  function status(text: string) {
    notice = text;
    drawInspector();
    window.setTimeout(() => {
      if (notice === text) {
        notice = "";
        drawInspector();
      }
    }, 4000);
  }

  // --- Rendering ---

  function visible(): Photo[] {
    const q = search.value.trim().toLowerCase();
    const list = photos.filter(
      (p) =>
        (!prefs.edited || p.edited) &&
        (!prefs.rawOnly || p.kind === "raw") &&
        p.rating >= prefs.minRating &&
        (prefs.flag === "any" || (prefs.flag === "unflagged" ? !p.flag : p.flag === prefs.flag)) &&
        (prefs.label === "any" || p.label === prefs.label) &&
        (!q || p.file_name.toLowerCase().includes(q)),
    );
    if (prefs.sort === "modified") list.sort((a, b) => b.modified_ms - a.modified_ms);
    else if (prefs.sort === "rating") list.sort((a, b) => b.rating - a.rating);
    return list;
  }

  function drawSide() {
    const item = (src: Source, name: string, count: string, tip: string, onDelete?: () => void) => {
      const b = h("button", { type: "button", title: tip }, h("span", { className: "name" }, name), h("span", { className: "count mono" }, count));
      b.classList.toggle("active", sameSource(src, source));
      b.addEventListener("click", () => void openSource(src));
      const li = h("li", {}, b);
      if (onDelete) {
        const del = h("button", { type: "button", className: "icon remove", title: `Delete ${name}`, textContent: "×" });
        del.addEventListener("click", (e) => {
          e.stopPropagation();
          if (!confirm(`Delete “${name}”? The photos themselves aren't touched.`)) return;
          onDelete();
          void saveCollections().then(() => (sameSource(src, source) ? openSource({ kind: "folder", path: prefs.recent[0]?.path ?? "" }, true) : drawSide()));
        });
        li.append(del);
      }
      return li;
    };
    recentList.replaceChildren(...prefs.recent.map((r) => item({ kind: "folder", path: r.path }, r.name, String(r.count), r.path)));
    if (prefs.recent.length === 0) recentList.append(h("li", { className: "muted none" }, "None yet"));
    albumList.replaceChildren(
      ...collections.albums.map((a) =>
        item({ kind: "album", name: a.name }, a.name, String(a.photos.length), "Album", () => {
          collections.albums = collections.albums.filter((x) => x !== a);
        }),
      ),
    );
    if (collections.albums.length === 0) albumList.append(h("li", { className: "muted none" }, "Select photos, then click +"));
    smartList.replaceChildren(
      ...collections.smart.map((c) =>
        item({ kind: "smart", name: c.name }, c.name, "", "Smart collection: photos in your recent folders that match its rules", () => {
          collections.smart = collections.smart.filter((x) => x !== c);
        }),
      ),
    );
    if (collections.smart.length === 0) smartList.append(h("li", { className: "muted none" }, "None yet"));
  }

  function drawGrid() {
    observer.disconnect();
    grid.style.setProperty("--tile", `${prefs.size}px`);
    const shown = visible();
    grid.replaceChildren(
      ...shown.map((p) => {
        const img = h("img", { alt: "", draggable: false });
        const tile = h("button", { type: "button", className: "tile", title: p.file_name }, img, h("span", { className: "badge kind mono" }, KIND_LABEL[p.kind]));
        tile.setAttribute("role", "option");
        tile.setAttribute("aria-label", [p.file_name, p.edited && "edited", p.rating && `${p.rating} stars`, p.flag, p.label && `${p.label} label`].filter(Boolean).join(", "));
        if (p.edited) tile.append(h("span", { className: "badge edited" }, "Edited"));
        if (p.rating) tile.append(h("span", { className: "badge rating" }, "★".repeat(p.rating)));
        if (p.flag) tile.append(h("span", { className: `badge flag ${p.flag}`, title: p.flag === "pick" ? "Pick" : "Rejected" }, p.flag === "pick" ? "⚑" : "✕"));
        if (p.label) tile.append(h("span", { className: `label-bar ${p.label}` }));
        tile.classList.toggle("rejected", p.flag === "reject");
        tile.dataset.path = p.path;
        tile.addEventListener("click", (e) => click(p.path, e));
        tile.addEventListener("dblclick", () => void actions.edit(p.path));
        observer.observe(tile);
        return tile;
      }),
    );
    markSelection();

    heading.textContent = source ? title : "Library";
    if (!source) counts.textContent = "";
    else {
      const raw = photos.filter((p) => p.kind === "raw").length;
      const other = photos.length - raw;
      const parts = [`${photos.length} photo${photos.length === 1 ? "" : "s"}`];
      if (raw && other) parts.push(`${raw} RAW`, `${other} other`);
      if (shown.length !== photos.length) parts.push(`showing ${shown.length}`);
      if (picked.size > 1) parts.push(`${picked.size} selected`);
      counts.textContent = parts.join(" · ");
    }
    bar.hidden = !source;
    empty.hidden = !!source && shown.length > 0;
    const choose = h("button", { type: "button", className: "primary", textContent: "Open folder…" });
    choose.addEventListener("click", () => void chooseFolder());
    if (!source) empty.replaceChildren(h("p", {}, "Open a folder to browse its photos."), choose);
    else if (photos.length === 0)
      empty.replaceChildren(
        h(
          "p",
          {},
          source.kind === "folder"
            ? "There are no photos Aperio can open in this folder."
            : source.kind === "album"
              ? "This album is empty. Select photos in a folder and use “Add to album”."
              : "No photos in your recent folders match this collection.",
        ),
        ...(source.kind === "folder" ? [choose] : []),
      );
    else empty.replaceChildren(h("p", { className: "muted" }, "No photos match the search or filters."));
  }

  function markSelection() {
    for (const t of grid.querySelectorAll<HTMLButtonElement>(".tile")) {
      const on = picked.has(t.dataset.path!);
      t.classList.toggle("selected", on);
      t.classList.toggle("primary", t.dataset.path === selected && picked.size > 1);
      t.setAttribute("aria-selected", String(on));
    }
  }

  function drawInspector() {
    const list = photos.filter((p) => picked.has(p.path));
    const p = photos.find((x) => x.path === selected);
    inspector.hidden = !p;
    if (!p) return;
    const img = h("img", { className: "thumb", alt: "" });
    thumbnail(p.path, (url) => (img.src = url), () => {});
    const note = notice ? [h("p", { className: "notice" }, notice)] : [];
    if (list.length > 1) {
      const others = list.filter((x) => x.path !== p.path).map((x) => x.path);
      inspector.replaceChildren(
        img,
        h("h3", {}, `${list.length} photos selected`),
        ...note,
        markControls(list),
        albumControls(list.map((x) => x.path)),
        syncControls(p, others),
      );
      return;
    }
    const facts = h("dl", { className: "facts" });
    const row = (label: string, value: string, mono = false) => facts.append(h("dt", {}, label), h("dd", { className: mono ? "mono" : "" }, value));
    row("Type", KIND_LABEL[p.kind]);
    row("File size", megabytes(p.size_bytes), true);
    row("Modified", new Date(p.modified_ms).toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }));
    row("Edit", p.edited ? `${p.file_name}.aperio.json` : "None yet");
    const extra = h("dl", { className: "facts" });
    const openIt = h("button", { type: "button", className: "primary", textContent: "Open in editor" });
    openIt.addEventListener("click", () => void actions.edit(p.path));
    const quick = h("button", { type: "button", textContent: "Quick export…" });
    quick.addEventListener("click", () => void actions.quickExport(p.path, urls.get(p.path)));
    inspector.replaceChildren(
      img,
      h("h3", {}, p.file_name),
      ...note,
      markControls([p]),
      extra,
      facts,
      albumControls([p.path]),
      h("p", { className: "hint muted" }, "Ctrl/⌘- or Shift-click to select more photos, then sync edits or rate them together."),
      h("span", { className: "spacer" }),
      openIt,
      quick,
    );

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
    [...ratingFilter.children].forEach((b, i) => b.classList.toggle("on", i < prefs.minRating));
    for (const b of flagFilter.querySelectorAll<HTMLButtonElement>("button")) b.classList.toggle("active", b.dataset.value === prefs.flag);
    for (const b of labelFilter.querySelectorAll<HTMLButtonElement>("button")) b.classList.toggle("on", b.dataset.value === prefs.label);
    drawSide();
    drawGrid();
    drawInspector();
  }

  function setPref(change: Partial<Prefs>) {
    prefs = { ...prefs, ...change };
    savePrefs();
    draw();
  }

  /** Select one photo, or with Ctrl/⌘ toggle it, or with Shift a range. */
  function click(path: string, e: MouseEvent | KeyboardEvent | null) {
    const shown = visible().map((p) => p.path);
    if (e?.shiftKey && anchor && shown.includes(anchor)) {
      const [a, b] = [shown.indexOf(anchor), shown.indexOf(path)].sort((x, y) => x - y);
      picked = new Set(shown.slice(a, b + 1));
      selected = path;
    } else if (e && (e.ctrlKey || e.metaKey)) {
      if (picked.has(path) && picked.size > 1) {
        picked.delete(path);
        if (selected === path) selected = [...picked][0];
      } else {
        picked.add(path);
        selected = path;
      }
      anchor = path;
    } else {
      picked = new Set([path]);
      selected = path;
      anchor = path;
    }
    markSelection();
    const tile = grid.querySelector<HTMLElement>(`.tile[data-path="${CSS.escape(path)}"]`);
    tile?.focus({ preventScroll: true });
    tile?.scrollIntoView({ block: "nearest" });
    counts.textContent = counts.textContent!.replace(/ · \d+ selected$/, "") + (picked.size > 1 ? ` · ${picked.size} selected` : "");
    drawInspector();
  }

  async function chooseFolder() {
    const picked = await open({ directory: true, multiple: false, defaultPath: source?.kind === "folder" ? source.path : undefined });
    if (typeof picked === "string") await openSource({ kind: "folder", path: picked });
  }

  search.addEventListener("input", drawGrid);

  /** Arrow keys move through the grid (with Shift, extending the
   * selection); Enter opens the photo. 0–5 rate, P/X/U flag, 6–9 label. */
  function key(e: KeyboardEvent) {
    const mod = e.ctrlKey || e.metaKey;
    const shown = visible();
    if (mod && e.key.toLowerCase() === "a") {
      e.preventDefault();
      picked = new Set(shown.map((p) => p.path));
      markSelection();
      drawGrid();
      drawInspector();
      return;
    }
    if (mod) return;
    const i = shown.findIndex((p) => p.path === selected);
    const columns = Math.max(1, getComputedStyle(grid).gridTemplateColumns.split(" ").length);
    const step: Record<string, number> = { ArrowLeft: -1, ArrowRight: 1, ArrowUp: -columns, ArrowDown: columns };
    const k = e.key.toLowerCase();
    if (e.key in step && shown.length > 0) {
      e.preventDefault();
      const next = Math.min(shown.length - 1, Math.max(0, (i < 0 ? 0 : i) + (i < 0 ? 0 : step[e.key])));
      click(shown[next].path, e.shiftKey ? e : null);
    } else if (e.key === "Enter" && selected) {
      e.preventDefault();
      void actions.edit(selected);
    } else if (/^[0-5]$/.test(e.key)) {
      e.preventDefault();
      void mark({ rating: Number(e.key) });
    } else if (e.key in LABEL_KEYS) {
      e.preventDefault();
      const label = LABEL_KEYS[e.key];
      const all = photos.filter((p) => picked.has(p.path)).every((p) => p.label === label);
      void mark({ label: all ? "none" : label });
    } else if (k === "p" || k === "x" || k === "u") {
      e.preventDefault();
      void mark({ flag: k === "p" ? "pick" : k === "x" ? "reject" : "none" });
    }
  }

  draw();
  const ready = invoke<Collections>("load_collections")
    .then((c) => (collections = { albums: c?.albums ?? [], smart: c?.smart ?? [] }))
    .catch(() => {})
    .then(() => drawSide());

  return {
    /** Show the library, re-reading the source so badges are current. */
    async show(edited?: string) {
      await ready;
      if (edited) forget(edited);
      if (source) await openSource(source);
      else if (prefs.recent[0]) await openSource({ kind: "folder", path: prefs.recent[0].path }, true);
      else draw();
    },
    openFolder: (path: string, quiet = false) => openSource({ kind: "folder", path }, quiet),
    chooseFolder,
    key,
  };
}
