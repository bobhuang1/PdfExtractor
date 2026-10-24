// The window is a thin shell over the CLI: every action here either calls a
// Tauri command or draws what the Rust side has already worked out. No
// conversion logic and no progress accounting live in this file.
//
// A run is streamed rather than awaited. `start_conversion` answers as soon as
// the CLI is running; the documents then arrive as `conversion-*` events, which
// is what keeps the window live and makes the Stop button mean something.

// Assigned in `boot`, which is also where a plain browser is handled.
let invoke;
let listen;

const el = (id) => document.getElementById(id);

// Ceilings, so a folder with thousands of documents cannot grow the page
// without bound.
const MAX_ROWS = 300;
const MAX_LOG_LINES = 2000;
// A comparison of a whole folder is many panels of many rows, and a panel is
// only useful while a person can still read it.
const MAX_DIFF_DOCUMENTS = 25;
const MAX_DIFF_ROWS = 400;

const EVENTS = [
  "conversion-started",
  "conversion-plan",
  "conversion-progress",
  "conversion-log",
  "conversion-finished",
  // Only sent when the Compare mode is on: one document beside its reference.
  "conversion-diff",
  // Only ever emitted by `pdfextractor-ui --selftest`.
  "selftest-enabled",
];

// The run on screen, or null before the first one. It is built from the
// `conversion-started` event rather than from the command's answer, so a run
// started by the self-test is drawn exactly like one started here.
let display = null;
let selftest = false;

function readSettings() {
  return {
    input: el("input").value.trim(),
    output: el("output").value.trim(),
    parallel: Number(el("parallel").value) || 1,
    recursive: el("recursive").checked,
    ocr: el("ocr").value,
    preview: el("preview").checked,
    // The mode switch decides whether a path is sent at all: an input left in
    // the box while Compare is off is not a request to compare.
    reference: el("compare").checked ? el("reference").value.trim() : "",
  };
}

function describe(settings) {
  const bits = [
    `parallel ${settings.parallel}`,
    settings.recursive ? "recursive" : "top level only",
    `OCR ${settings.ocr}`,
  ];
  if (settings.preview) bits.push("preview");
  if (settings.reference) bits.push(`compare: ${baseName(settings.reference)}`);
  return bits.join(" · ");
}

function refreshSummary() {
  if (display && !display.finished) return;
  const settings = readSettings();
  el("summary").textContent = settings.input ? describe(settings) : "";
}

function baseName(path) {
  const parts = path.split(/[\\/]/);
  return parts[parts.length - 1] || path;
}

// ---- the page ------------------------------------------------------------

function setBar(fraction) {
  const fill = el("bar-fill");
  if (fraction === null) {
    // No count yet: a bar that sweeps rather than one that lies about progress.
    fill.classList.add("indeterminate");
    return;
  }
  fill.classList.remove("indeterminate");
  fill.style.width = `${Math.max(0, Math.min(100, fraction * 100))}%`;
}

function appendLog(line) {
  const log = el("log");
  // Only follow the output when the reader is already at the end of it, so
  // scrolling back to read a failure is not yanked away.
  const atBottom = log.scrollTop + log.clientHeight >= log.scrollHeight - 4;
  log.appendChild(document.createTextNode(`${line}\n`));
  while (log.childNodes.length > MAX_LOG_LINES) {
    log.removeChild(log.childNodes[0]);
  }
  if (atBottom) log.scrollTop = log.scrollHeight;
}

function showMessage(text) {
  const box = el("result");
  box.classList.remove("hidden", "ok", "warn");
  box.classList.add("bad");
  el("result-title").textContent = "Cannot convert";
  el("command").textContent = "";
  el("command").hidden = true;
  el("verdict").textContent = text;
  el("summary").textContent = "";
}

function showProgress() {
  el("run").classList.remove("hidden");
  el("files").replaceChildren();
  el("log").replaceChildren();
  el("result").classList.add("hidden");
  el("compare-result").classList.add("hidden");
  el("diffs").replaceChildren();
  delete el("diffs").dataset.more;
  el("run-state").textContent = "Starting…";
  el("run-command").textContent = "";
  setBar(null);
}

function updateState() {
  const total = display.total ? String(display.total) : "?";
  const parts = [`${display.done} / ${total} documents`];
  if (display.converted) parts.push(`${display.converted} converted`);
  if (display.failed) parts.push(`${display.failed} failed`);
  if (display.finished) parts.push("done");
  el("run-state").textContent = parts.join(" · ");
}

function newRow(path) {
  const row = document.createElement("li");
  const name = document.createElement("span");
  name.className = "name";
  name.textContent = baseName(path);
  name.title = path;
  const status = document.createElement("span");
  status.className = "status";
  status.textContent = "queued";
  row.appendChild(name);
  row.appendChild(status);
  return { row, status };
}

function handlePlan(total, inputs) {
  display.total = total || inputs.length;
  display.rows = new Map();
  const list = el("files");
  list.replaceChildren();
  for (const path of inputs.slice(0, MAX_ROWS)) {
    const entry = newRow(path);
    list.appendChild(entry.row);
    display.rows.set(path, entry);
  }
  if (inputs.length > MAX_ROWS) {
    const more = document.createElement("li");
    more.className = "more";
    more.textContent = `…and ${inputs.length - MAX_ROWS} more documents`;
    list.appendChild(more);
  }
  setBar(0);
  updateState();
}

function handleProgress(progress) {
  display.done = progress.done;
  if (progress.total) display.total = progress.total;
  if (progress.ok) display.converted += 1;
  else display.failed += 1;

  const entry = display.rows.get(progress.input);
  if (entry) {
    entry.row.classList.add(progress.ok ? "ok" : "bad");
    entry.status.textContent = progress.ok ? "converted" : "failed";
    entry.status.title = progress.output || progress.error || "";
    if (progress.error) entry.row.dataset.error = progress.error;
  }

  setBar(display.total ? display.done / display.total : null);
  updateState();
}

function verdictFor(finished) {
  const converted = finished.tally.converted;
  const total = display.total;
  let text = `${converted} of ${total} document(s) converted`;
  if (finished.tally.failed) text += `, ${finished.tally.failed} failed`;
  text += ".";
  if (finished.cancelled) {
    text += " Stopped by the user; documents already written were kept.";
  } else if (finished.exitCode) {
    text += ` The command exited with ${finished.exitCode}.`;
  }
  return text;
}

// ---- the comparison ------------------------------------------------------

// The page's whole knowledge of the Compare mode is one event: what arrived is
// what is drawn, so a document the backend could not compare says why rather
// than showing an empty panel.

// One block, as a cell's contents: what it is on its own line, so the text gets
// the width of the cell however long the label happens to be.
function diffBlock(row, block) {
  const box = document.createElement("div");
  box.className = "block";
  const head = document.createElement("span");
  head.className = "head";
  // The badge earns its place only when the kind says something the label does
  // not: our pictures are labelled "picture" already, but a disagreement about
  // what a block *is* is worth seeing.
  if (row.kind !== "text" && row.kind !== block.label) {
    const kind = document.createElement("span");
    kind.className = `kind ${row.kind}`;
    kind.textContent = row.kind;
    head.appendChild(kind);
  }
  const label = document.createElement("span");
  label.className = "label";
  label.textContent = block.label;
  head.appendChild(label);
  if (block.page !== undefined) {
    const page = document.createElement("span");
    page.className = "page";
    page.textContent = `p${block.page}`;
    head.appendChild(page);
  }
  box.appendChild(head);
  const text = document.createElement("span");
  text.className = "text";
  text.textContent = block.text || "(empty)";
  box.appendChild(text);
  return box;
}

// An item with nothing that could be read as a word in it: a dotted leader, a rule.
function letterless(block) {
  return !/[a-z0-9]/i.test(block.text || "");
}

// A cell holds every block that side has here. One, usually — but the reference
// writes an item per line, so a paragraph we read as a single block is a *run* of
// its items inside it, and the whole run is drawn rather than the row pretending
// to be the first of them.
function diffCell(row, side) {
  const blocks = row[side] || [];
  const cell = document.createElement("div");
  cell.className = "cell";
  if (!blocks.length) {
    cell.classList.add("missing");
    cell.textContent = "—";
    cell.title = side === "ours" ? "not in our output" : "not in the reference";
    return cell;
  }
  // A run of items with no letters in them — the dots of a form's leader and the rule
  // under them — is one thing on the page, so it is drawn as one block naming how many,
  // rather than as an empty box each. The row's note says the same in words.
  if (blocks.length > 1 && blocks.every(letterless)) {
    const first = blocks[0];
    cell.appendChild(
      diffBlock(row, {
        label: first.label,
        text: `${blocks.length} items with no letters`,
        page: first.page,
      })
    );
    return cell;
  }
  for (const block of blocks) cell.appendChild(diffBlock(row, block));
  return cell;
}

// How wide the page drawing beside a single placed row is. Small on purpose: it is
// there to answer *where* on the page an item lies, not to be studied.
const THUMB_WIDTH = 46;

// How wide a per-page drawing is. Bigger, because it is there to be *looked at*: the
// leader pattern of a form is the finding, and one thumbnail per row cannot show it.
const PAGE_WIDTH = 138;

// A box as a page drawing needs it: a share of the page for a style, and a point
// measured on the page for a caption. Shared by the row thumbnail and the page view, so
// the two draw one box the same way rather than each doing its own sum.
const share = (fraction) => `${(fraction * 100).toFixed(2)}%`;
const points = (fraction, size) => (fraction * size).toFixed(0);
const boxPoints = (box, width, height) =>
  `${points(box.x, width)},${points(box.y, height)}–${points(
    box.x + box.w,
    width
  )},${points(box.y + box.h, height)}`;

// A box drawn on a page: a share of the page in each direction, kept visible however
// thin a run is, since a dotted leader is a very small share of a page's height.
function placeMark(cls, box) {
  const mark = document.createElement("span");
  mark.className = cls;
  mark.style.left = share(box.x);
  mark.style.top = share(box.y);
  mark.style.width = share(box.w);
  mark.style.height = share(box.h);
  return mark;
}

function diffRow(row) {
  const line = document.createElement("li");
  line.className = `row ${row.verdict}`;
  if (row.note) line.title = row.note;
  line.appendChild(diffCell(row, "ours"));
  line.appendChild(diffCell(row, "theirs"));
  if (row.note || row.placement) {
    const foot = document.createElement("div");
    foot.className = "foot";
    // A placed row is drawn as well as told: a placement is about where something
    // is, and a box on the page's own shape says that at a glance where the note's
    // coordinates have to be read.
    if (row.placement) foot.appendChild(pageThumb(row.placement));
    if (row.note) {
      const note = document.createElement("p");
      note.className = "note";
      note.textContent = row.note;
      foot.appendChild(note);
    }
    line.appendChild(foot);
  }
  return line;
}

// The page an item lies on, drawn to its own shape with the item's box on it. The
// backend sends the box as fractions of the page it was found on, so no page image is
// needed — which the comparison has no way to send — to show where it lies.
function pageThumb(placement) {
  const thumb = document.createElement("span");
  thumb.className = "thumb";
  thumb.style.width = `${THUMB_WIDTH}px`;
  thumb.style.height = `${Math.max(
    Math.round((THUMB_WIDTH * placement.height) / placement.width),
    8
  )}px`;
  const box = boxPoints(placement, placement.width, placement.height);
  thumb.title = `page ${placement.page}: box ${box}`;
  thumb.setAttribute("role", "img");
  thumb.setAttribute("aria-label", `the same page: an item with no letters in it at box ${box}`);
  thumb.appendChild(placeMark("thumb-box", placement));
  return thumb;
}

// One page with every placed item on it at once, so a form's leader pattern is one
// picture rather than one thumbnail per row. The backend groups the items by the page
// they lie on and sends each box as a share of that page, so no image of it is needed —
// which the comparison has no way to send.
function pageView(page) {
  const width = PAGE_WIDTH;
  const height = Math.max(Math.round((width * page.height) / page.width), 40);
  const figure = document.createElement("figure");
  figure.className = "page-map";
  const sheet = document.createElement("span");
  sheet.className = "sheet";
  sheet.style.width = `${width}px`;
  sheet.style.height = `${height}px`;
  const where = page.boxes
    .map((box) => boxPoints(box, page.width, page.height))
    .join("; ");
  sheet.title = `page ${page.page}: ${page.boxes.length} placed item(s) at box ${where}`;
  sheet.setAttribute("role", "img");
  sheet.setAttribute(
    "aria-label",
    `page ${page.page}: the ${page.boxes.length} item(s) with no letters in them, drawn where they lie`
  );
  for (const box of page.boxes) sheet.appendChild(placeMark("mark", box));
  figure.appendChild(sheet);
  const caption = document.createElement("figcaption");
  caption.textContent = `page ${page.page} · ${page.boxes.length} item(s)`;
  figure.appendChild(caption);
  return figure;
}

// The places of a document, page by page: every placed item of a page drawn together.
// The backend builds this from the rows *before* they are cut to what the panel lists,
// so a long form's page is whole even where its rows are not.
function placedPagesView(pages) {
  const section = document.createElement("section");
  section.className = "placed-pages";
  const items = pages.reduce((count, page) => count + page.boxes.length, 0);
  const title = document.createElement("p");
  title.className = "meta";
  title.textContent =
    `${pages.length} page(s) carry ${items} placed item(s) — every one drawn where it ` +
    `lies, so a form's leader pattern is one picture rather than one box per row:`;
  section.appendChild(title);
  const gallery = document.createElement("div");
  gallery.className = "page-gallery";
  for (const page of pages) gallery.appendChild(pageView(page));
  section.appendChild(gallery);
  return section;
}

// What the panel says *instead of* a comparison, when the memory beside the reference
// remembers a form this reading has lost items from. Nothing of the comparison is drawn:
// a list of rows from a reading whose panel has lost the form would invite a reader to
// review differences that are beside the point.
function floorBanner(floor) {
  const box = document.createElement("p");
  box.className = "floor";
  box.textContent =
    `Below the floor the memory beside the reference remembers: ${floor}. ` +
    `This reading places fewer of the reference's items than compare-baseline.json ` +
    `remembers, so the comparison is withheld rather than drawn — a form's leader ` +
    `pattern leaving the panel is news about the reading, not a ` +
    `difference to review. Fix the reading, or accept the trade by regenerating the ` +
    `memory and reading its diff (CORPUS_BASELINE=update cargo test -p pdfextractor-ui ` +
    `--test corpus_report_test).`;
  return box;
}

function diffPanel(report) {
  const panel = document.createElement("details");
  panel.className = "diff";

  const summary = report.summary || {};
  // The floor this document fell through, in words, or empty when it is at or above it.
  const floor = report.floor || "";
  // The joined count is shown only when there is one: unlike the verdicts it is
  // not a property of the document but of how much the two readings disagree
  // about order, and a permanent "0" would be noise on every panel.
  const counts = report.problem
    ? report.problem
    : `${summary.same} same · ${summary.notation} notation · ${summary.differs} differ · ` +
      `${summary.contained} contained · ${summary.oursOnly} only ours · ` +
      `${summary.theirsOnly} only the reference` +
      (summary.placed ? ` · ${summary.placed} placed` : "") +
      (summary.pairedByPosition ? ` · ${summary.pairedByPosition} paired by position` : "");

  const head = document.createElement("summary");
  const name = document.createElement("span");
  name.className = "name";
  name.textContent = baseName(report.input);
  name.title = report.input;
  const verdict = document.createElement("span");
  verdict.className = "counts";
  verdict.textContent = report.problem
    ? "not compared"
    : floor
    ? `below the floor · ${floor}`
    : counts;
  head.appendChild(name);
  head.appendChild(verdict);
  panel.appendChild(head);

  // What the self-test reads back: the numbers as rendered, not as told.
  panel.dataset.agreement = report.problem
    ? ""
    : `${(summary.textAgreement * 100).toFixed(1)}%`;
  // The self-test reads the *floor* off the panel as well: the words the memory's rule
  // produced, exactly as the ratchet prints them, or nothing on a document at the floor.
  panel.dataset.floor = floor;
  // …and which rules held the document: "ratchet" (every rule — the run's settings match
  // the memory's), "floor" (only the floor under a form's placed items), or nothing where
  // the memory had nothing to hold it to.
  panel.dataset.held = report.held ? report.held.rules : "";
  panel.dataset.verdict = report.problem
    ? "no-reference"
    : floor
    ? "below-floor"
    : `same=${summary.same} notation=${summary.notation} differs=${summary.differs}` +
      ` contained=${summary.contained}` +
      ` ours-only=${summary.oursOnly} theirs-only=${summary.theirsOnly}` +
      ` placed=${summary.placed}` +
      ` paired-by-position=${summary.pairedByPosition}` +
      ` truncated=${summary.truncated}`;

  const meta = document.createElement("p");
  meta.className = "meta";
  const reference = document.createElement("code");
  reference.textContent = report.reference;
  meta.appendChild(document.createTextNode("reference: "));
  meta.appendChild(reference);
  if (!report.problem) {
    meta.appendChild(
      document.createTextNode(
        ` · ${((summary.textAgreement || 0) * 100).toFixed(1)}% text agreement` +
          ` · ${summary.pagesOurs}/${summary.pagesTheirs} pages`
      )
    );
    if (summary.truncated) {
      meta.appendChild(
        document.createTextNode(` · ${summary.truncated} block(s) not compared`)
      );
    }
  }
  panel.appendChild(meta);

  // Which of the ratchet's rules held this document, and why: the memory beside the
  // reference either speaks for this run's settings — every rule — or cannot, and only
  // the floor under a form's placed items is left. Which of the two it was is a fact
  // about the *run*, so it is on the panel rather than left to be guessed from two
  // settings screens — and it is on a refused panel too, saying whether the whole
  // ratchet refused the document or the floor alone.
  // The class is just `held`, never the rules' own name: `floor` is the class of the
  // refusal banner, and a held-to-the-floor-alone panel is not a refused one.
  if (report.held) {
    const held = document.createElement("p");
    held.className = "held";
    held.textContent = `Held to the memory: ${report.held.why}`;
    panel.appendChild(held);
  }

  // What the comparison noticed beyond the rows: a page one side has and the
  // other does not, a page measured differently, work left out of a big document.
  for (const note of summary.notes || []) {
    const line = document.createElement("p");
    line.className = "meta";
    line.textContent = note;
    panel.appendChild(line);
  }

  // A document below the floor is refused: the panel says what fell and stops, rather
  // than drawing a comparison from a reading the memory has already called lost. The
  // counts the self-test reads are set to nothing drawn, because nothing was.
  if (floor) {
    panel.appendChild(floorBanner(floor));
    panel.dataset.placedDrawn = "0";
    panel.dataset.placedPages = "0";
    panel.dataset.rowsDrawn = "0";
    return panel;
  }

  if (!report.problem) {
    // The places of the document before its rows: a form's leader pattern is one picture
    // to look at rather than a scattering of thumbnails to scroll past, and the page is
    // whole where the rows may not be.
    if (report.placedPages && report.placedPages.length) {
      panel.appendChild(placedPagesView(report.placedPages));
    }

    const bar = document.createElement("div");
    bar.className = "columns";
    for (const title of ["Our extractor", "Docling reference"]) {
      const column = document.createElement("span");
      column.textContent = title;
      bar.appendChild(column);
    }
    panel.appendChild(bar);

    const rows = document.createElement("ul");
    rows.className = "diff-rows";
    for (const row of (report.rows || []).slice(0, MAX_DIFF_ROWS)) {
      rows.appendChild(diffRow(row));
    }
    if (report.rows && report.rows.length > MAX_DIFF_ROWS) {
      const more = document.createElement("li");
      more.className = "more";
      more.textContent = `…and ${report.rows.length - MAX_DIFF_ROWS} more rows`;
      rows.appendChild(more);
    }
    panel.appendChild(rows);
    // How many placed boxes the panel actually drew. Counted from the DOM rather
    // than from the summary, so the self-test reads a drawing that is there rather
    // than one that was asked for. It is therefore the smaller of the summary's
    // `placed` and what fits in the rows above (`MAX_DIFF_ROWS`, the rows of a long
    // document that are shown at all), minus any placement of a page whose size the
    // reference never sent — those have no outline to draw the box on.
    panel.dataset.placedDrawn = String(rows.querySelectorAll(".thumb").length);
    // How many rows were drawn at all: a panel that refused the comparison draws none,
    // which is how the self-test tells a withheld document from a shown one.
    panel.dataset.rowsDrawn = String(rows.children.length);
    // How many page pictures the panel drew, counted from the DOM for the same reason:
    // the self-test reads a drawing that is there rather than one that was asked for.
    panel.dataset.placedPages = String(panel.querySelectorAll(".page-map").length);
  }
  return panel;
}

function handleDiff(report) {
  const box = el("compare-result");
  box.classList.remove("hidden");
  const list = el("diffs");
  const panel = diffPanel(report);
  // A document whose reading differs is the one worth opening: the others are
  // there to be folded away. A row paired by position opens it too — the two
  // readings may read the same way and still disagree about order, and that
  // disagreement is on the rows.
  panel.open = Boolean(report.problem) || Boolean(report.floor) ||
    report.summary.differs > 0 ||
    report.summary.contained > 0 || report.summary.oursOnly > 0 ||
    report.summary.theirsOnly > 0 || report.summary.pairedByPosition > 0;
  list.appendChild(panel);
  while (list.childElementCount > MAX_DIFF_DOCUMENTS) {
    list.removeChild(list.firstChild);
  }
  if (list.dataset.more !== "1" && list.childElementCount === MAX_DIFF_DOCUMENTS) {
    list.dataset.more = "1";
    const more = document.createElement("p");
    more.className = "meta";
    more.id = "diffs-more";
    more.textContent = `Only the last ${MAX_DIFF_DOCUMENTS} comparisons are shown.`;
    list.appendChild(more);
  }
}

/// The comparison state as *rendered*, which is what the self-test checks.
function diffSummary() {
  const panels = [...document.querySelectorAll("#diffs .diff")];
  if (!panels.length) return "diffs=0";
  const first = panels[0];
  return `diffs=${panels.length} diff-agreement="${first.dataset.agreement}"` +
    ` diff-verdict="${first.dataset.verdict}"` +
    ` placed-drawn=${first.dataset.placedDrawn}` +
    ` placed-pages=${first.dataset.placedPages}` +
    ` diff-rows=${first.dataset.rowsDrawn}` +
    ` floors=${panels.filter((panel) => panel.dataset.floor).length}` +
    (first.dataset.floor ? ` floor="${first.dataset.floor}"` : "") +
    (first.dataset.held ? ` held=${first.dataset.held}` : "");
}

function handleFinished(finished) {
  display.finished = true;
  display.converted = finished.tally.converted;
  display.failed = finished.tally.failed;
  display.total = finished.tally.total || display.total ||
    display.converted + display.failed;
  display.done = display.converted + display.failed;
  setBar(display.total ? display.done / display.total : 1);
  updateState();

  const box = el("result");
  box.classList.remove("hidden", "ok", "bad", "warn");
  box.classList.add(finished.cancelled ? "warn" : finished.success ? "ok" : "bad");
  el("result-title").textContent = finished.cancelled
    ? "Stopped"
    : finished.success
    ? "Finished"
    : "Failed";
  el("command").textContent = display.command;
  el("command").hidden = false;
  el("verdict").textContent = verdictFor(finished);

  display.outcome = finished;
  el("convert").disabled = false;
  el("stop").classList.add("hidden");
  el("stop").disabled = false;
  el("stop").textContent = "Stop";
  refreshSummary();

  if (selftest) {
    invoke("selftest_rendered", { summary: renderedSummary() }).catch((error) =>
      console.error(error)
    );
  }
}

// ---- events --------------------------------------------------------------

function beginRun(started) {
  display = {
    id: started.run,
    command: started.command,
    total: 0,
    done: 0,
    converted: 0,
    failed: 0,
    rows: new Map(),
    finished: false,
  };
  showProgress();
  el("convert").disabled = true;
  el("stop").classList.remove("hidden");
  el("stop").disabled = false;
  el("stop").textContent = "Stop";
  // What is being run, available while it runs rather than only once it is over.
  el("run-command").textContent = started.command;
  el("summary").textContent = "Running…";
}

/// What the self-test asks the page to hand back: the state as *rendered*, not
/// as told, so a mistake in the drawing is not mistaken for a working window.
function renderedSummary() {
  const rows = [...document.querySelectorAll("#files li")].map((li) => {
    const status = li.querySelector(".status");
    return status ? status.textContent : "?";
  });
  return [
    `run=${display.id}`,
    `state="${el("run-state").textContent}"`,
    `title="${el("result-title").textContent}"`,
    `bar="${el("bar-fill").style.width}"`,
    `success=${display.outcome ? display.outcome.success : "?"}`,
    `cancelled=${display.outcome ? display.outcome.cancelled : "?"}`,
    `verdict="${el("verdict").textContent}"`,
    `command="${el("run-command").textContent}"`,
    `rows=[${rows.join(",")}]`,
    // How many formats the CLI named, and the headings they were grouped under,
    // as rendered: the ask round-tripped and the page used its families.
    `formats=${el("formats").dataset.count || "?"}`,
    `families="${[...document.querySelectorAll("#format-families dt")]
      .map((heading) => heading.textContent)
      .join("|")}"`,
    // The binary named beside them, after the re-ask a self-test makes.
    `cli="${el("cli-program").textContent}"`,
    diffSummary(),
    `logs=${el("log").childNodes.length}`,
  ].join(" ");
}

function handle(kind, payload) {
  if (kind === "conversion-plan") return handlePlan(payload.total, payload.inputs);
  if (kind === "conversion-progress") return handleProgress(payload);
  if (kind === "conversion-log") return appendLog(payload.line);
  if (kind === "conversion-diff") return handleDiff(payload);
  if (kind === "conversion-finished") return handleFinished(payload);
}

function dispatch(kind, payload) {
  if (kind === "selftest-enabled") {
    selftest = true;
    // The smoke test asks the CLI again on the path a person uses after replacing
    // the binary: `formats=` in its report is then the refresh's answer, so the
    // re-ask is checked end to end with the rest of the window.
    reaskFormats();
    return;
  }
  if (!payload) return;
  if (kind === "conversion-started") {
    // Always the first event of a run, which is what makes the run on screen
    // authoritative rather than the answer to the call that started it.
    beginRun(payload);
    return;
  }
  if (!display || payload.run !== display.id) return; // an older run's news
  handle(kind, payload);
}

// ---- actions -------------------------------------------------------------

async function start() {
  const settings = readSettings();
  if (!settings.input) {
    showMessage("Choose an input file or folder.");
    return;
  }
  const previous = display;
  el("summary").textContent = "Starting…";
  showProgress();

  try {
    // The answer only says the process started; the run itself is drawn from
    // the events that follow.
    await invoke("start_conversion", { settings });
  } catch (error) {
    if (previous && !previous.finished) {
      // Another conversion is still going: keep it on screen and say so.
      appendLog(`cannot start another conversion: ${error}`);
      el("convert").disabled = true;
      return;
    }
    display = null;
    el("convert").disabled = false;
    el("stop").classList.add("hidden");
    el("run").classList.add("hidden");
    showMessage(String(error));
  }
}

async function stop() {
  el("stop").disabled = true;
  el("stop").textContent = "Stopping…";
  try {
    const stopped = await invoke("cancel_conversion");
    if (!stopped) {
      // Nothing was running after all; the finish event will tidy up.
      el("stop").disabled = false;
      el("stop").textContent = "Stop";
    }
  } catch (error) {
    appendLog(`could not stop the conversion: ${error}`);
    el("stop").disabled = false;
    el("stop").textContent = "Stop";
  }
}

// The formats on offer are the CLI's, not this page's: `input_formats` runs
// `pdfextractor-cli --list-input-formats`, so what is named under the input box
// is the set a run will actually take. The native chooser is filtered by the same
// answer on the Rust side; nothing here repeats it.
//
// The answer carries the families too, under the CLI's own names. Drawing them
// here is what keeps the page's headings the chooser's headings rather than a
// second grouping written on this side.
function showFamilies(families) {
  const rows = document.createDocumentFragment();
  for (const family of families) {
    const row = document.createElement("div");
    row.className = "format-family";
    const heading = document.createElement("dt");
    heading.textContent = family.name;
    const formats = document.createElement("dd");
    formats.textContent = family.inputFormats.join(", ");
    row.append(heading, formats);
    rows.append(row);
  }
  el("format-families").replaceChildren(rows);
}

// One function draws the answer for both asks, so the list on load and the list
// after "Ask again" cannot disagree about how they are written.
function showFormats(answer) {
  const hint = el("formats");
  const families = answer.families.length;
  const label = families === 1 ? "family" : "families";
  hint.textContent = `Reads ${answer.names.length} format(s) in ${families} ${label}`;
  hint.dataset.count = String(answer.names.length);
  hint.classList.remove("bad");
  showFamilies(answer.families);
}

function showFormatsProblem(error) {
  // The same call a conversion would make first, so a missing or broken CLI is
  // said here rather than only when a run is attempted. The old headings are
  // cleared with it: they were the previous binary's, not this answer's.
  const hint = el("formats");
  hint.textContent = `Could not ask the CLI which formats it reads: ${error}`;
  hint.dataset.count = "0";
  hint.classList.add("bad");
  el("format-families").replaceChildren();
}

// The binary the formats came from, named beside them. It is asked of the
// backend separately because the environment can move between a refresh's
// format ask and this call; the backend resolves it again, so the line names the
// path a run would start now rather than the one the window opened with.
async function loadCliProgram() {
  const line = el("cli-line");
  const program = el("cli-program");
  try {
    const path = await invoke("cli_program");
    program.textContent = path;
    program.title = path;
    line.classList.remove("bad");
  } catch (error) {
    program.textContent = `could not resolve the binary: ${error}`;
    program.removeAttribute("title");
    line.classList.add("bad");
  }
}

async function loadFormats() {
  try {
    showFormats(await invoke("input_formats"));
  } catch (error) {
    showFormatsProblem(error);
  }
  await loadCliProgram();
}

// Ask again rather than reopen the window: the binary a run starts can be
// rebuilt, replaced or repaired while this window is open, and the answer was
// asked once on load and kept. `refresh_input_formats` drops that answer and
// asks the CLI that is there now; the chooser's filter is the same kept answer
// on the Rust side, so it moves with the line.
async function reaskFormats() {
  const button = el("reask-formats");
  button.disabled = true;
  el("formats").textContent = "Asking the CLI again which formats it reads…";
  el("formats").classList.remove("bad");
  el("format-families").replaceChildren();
  try {
    showFormats(await invoke("refresh_input_formats"));
  } catch (error) {
    showFormatsProblem(error);
  } finally {
    button.disabled = false;
    await loadCliProgram();
  }
}

async function pick(kind, target) {
  const path = await invoke("pick_path", { kind });
  if (path) {
    el(target).value = path;
  }
  return path;
}

// ---- boot ----------------------------------------------------------------

function wire() {
  // "document" is the chooser filtered by the CLI's own formats; a reference is
  // any file, so it is picked without a filter.
  el("pick-file").addEventListener("click", () => pick("document", "input"));
  el("pick-folder").addEventListener("click", () => pick("folder", "input"));
  el("pick-output").addEventListener("click", () => pick("folder", "output"));

  el("compare").addEventListener("change", () => {
    // The mode owns the target: a disabled field cannot be picked into, and a
    // comparison that is off is not half-off.
    const on = el("compare").checked;
    for (const id of ["reference", "pick-reference-file", "pick-reference-folder"]) {
      el(id).disabled = !on;
    }
    document.querySelector("main").classList.toggle("comparing", on);
    if (on && !el("reference").value) el("reference").focus();
    refreshSummary();
  });
  el("pick-reference-file").addEventListener("click", () => pick("file", "reference"));
  el("pick-reference-folder").addEventListener("click", () => pick("folder", "reference"));
  el("reference").addEventListener("input", refreshSummary);

  el("parallel").addEventListener("input", () => {
    el("parallel-value").textContent = el("parallel").value;
    refreshSummary();
  });

  for (const id of ["input", "output"]) {
    el(id).addEventListener("input", refreshSummary);
  }
  for (const id of ["recursive", "preview", "ocr"]) {
    el(id).addEventListener("change", refreshSummary);
  }

  el("convert").addEventListener("click", start);
  el("stop").addEventListener("click", stop);
  el("reask-formats").addEventListener("click", reaskFormats);

  refreshSummary();
}

async function boot() {
  invoke = window.__TAURI__.core.invoke;
  listen = window.__TAURI__.event.listen;
  for (const kind of EVENTS) {
    await listen(kind, (event) => dispatch(kind, event.payload));
  }
  wire();
  // Awaited so a self-test sees the list drawn, and so a CLI that cannot answer
  // is said on the page before anything is attempted. The ask only prints and
  // exits; this is not waiting on a conversion.
  await loadFormats();
  // Tell the backend the page mounted and the IPC works. With `--selftest` the
  // app exits on this call, which is how the stack is smoke-tested.
  invoke("frontend_ready").catch((error) => console.error(error));
}

if (window.__TAURI__) {
  boot();
} else if (new URLSearchParams(window.location.search).has("mock")) {
  // A plain browser has no Tauri API. `?mock=1` loads the development stand-in
  // (mock.js) so this page can be exercised outside the desktop app.
  const mock = document.createElement("script");
  mock.src = "mock.js";
  mock.addEventListener("load", boot);
  document.head.appendChild(mock);
} else {
  el("summary").textContent = "This page runs inside the desktop app.";
}
