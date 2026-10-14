// A stand-in for the Tauri API, loaded only by a page opened with `?mock=1` in
// a plain browser, so this interface can be exercised without building the
// desktop app. The desktop window never loads this file.
//
// It plays a short script: three documents, the middle one failing, a stop that
// takes effect wherever the play has got to, and — when the Compare mode is on —
// a comparison of the first document with a reference: one block different, one
// block only we have, a drawing the two readings ordered differently, which the
// backend joined by its page and box rather than by its content, and a form's
// dotted leader placed where our reading read no line at all — and the placed items of
// each page drawn together, one picture per page. The plan
// and the first log line are emitted *before* `start_conversion` answers on
// purpose, which is the narrow race the page has to survive.

(() => {
  const listeners = new Map();
  const emit = (event, payload) => {
    for (const handler of listeners.get(event) || []) handler({ event, payload });
  };

  const DOCUMENTS = ["/docs/annual report.pdf", "/docs/notes.md", "/docs/table.csv"];
  const STEP_MS = 600;

  let run = 0;
  let stopped = false;
  let timer = null;
  // Set by `start_conversion`: the stand-in plays a comparison only when the
  // page asked for one, exactly as the backend does.
  let comparing = false;

  const block = (kind, label, text, page) => ({ kind, label, text, page });

  /// The comparison the backend would send for the first document: the same
  /// blocks, one read as a different kind of heading, one only we found, one
  /// drawing the two readings each put in the other's place, and one paragraph the
  /// reference read as a run of lines (so the page's run rendering is exercised).
  ///
  /// A row's two sides are *lists* — what the backend sends, and what the page
  /// draws: one block of ours against a run of the reference's items.
  function diff(id, input) {
    return {
      run: id,
      input,
      reference: "/golden/docling/annual report.json",
      // Which of the ratchet's rules the memory beside the reference held this
      // document to, and why — the stand-in's run is measured with `--ocr auto`
      // (see the command above) against a memory measured with `--ocr never`, so
      // only the floor under a form's placed items could apply. The page shows the
      // sentence under the panel's header.
      held: {
        rules: "floor",
        why: "only the floor under a form's placed items applies: this run was measured with --ocr auto and the memory with --ocr never, and OCR moves the counts that measure how the two readings agree",
      },
      summary: {
        rows: 7,
        same: 2,
        notation: 0,
        differs: 1,
        contained: 1,
        oursOnly: 1,
        theirsOnly: 0,
        placed: 2,
        pairedByPosition: 1,
        textAgreement: 0.982,
        pagesOurs: 2,
        pagesTheirs: 2,
        truncated: 0,
        notes: [],
      },
      // Every placed item of each page, drawn at once: the per-page view the panel puts
      // above its rows. Two pages, so a page carrying a whole pattern and a page
      // carrying one line are both exercised.
      placedPages: [
        {
          page: 1,
          width: 612,
          height: 792,
          boxes: [
            { x: 50 / 612, y: 560 / 792, w: 250 / 612, h: 20 / 792 },
            { x: 300 / 612, y: 560 / 792, w: 250 / 612, h: 20 / 792 },
            { x: 50 / 612, y: 580 / 792, w: 500 / 612, h: 1 / 792 },
          ],
        },
        {
          page: 2,
          width: 612,
          height: 792,
          boxes: [
            { x: 60 / 612, y: 120 / 792, w: 200 / 612, h: 20 / 792 },
            { x: 300 / 612, y: 120 / 792, w: 240 / 612, h: 20 / 792 },
          ],
        },
      ],
      rows: [
        // Two one-sided rows the backend would have sent as "only ours" and
        // "only the reference", joined because both readings put the same
        // drawing in the same box.
        {
          kind: "picture",
          verdict: "same",
          ours: [block("picture", "picture", "image/png 480x320 @144dpi", 1)],
          theirs: [block("picture", "picture", "image/png 480x320 @144dpi", 1)],
          note: "paired by position: page 1, 91% of the smaller box shared",
        },
        {
          kind: "text",
          verdict: "same",
          ours: [block("text", "title", "Annual report 2025", 1)],
          theirs: [block("text", "title", "Annual report 2025", 1)],
        },
        {
          kind: "text",
          verdict: "differs",
          ours: [block("text", "section_header", "Where the money went", 1)],
          theirs: [block("text", "paragraph", "Where the money went", 1)],
          note: "label: section_header vs paragraph",
        },
        // One block of ours beside the run of reference items wholly inside it:
        // the words are all there and the two readings cut them differently.
        {
          kind: "text",
          verdict: "contained",
          ours: [
            block(
              "text",
              "paragraph",
              "The board recommends a final dividend of 12p per share.",
              1
            ),
          ],
          theirs: [
            block("text", "text", "The board recommends a final dividend", 1),
            block("text", "text", "of 12p per share.", 1),
          ],
          note: "2 of the reference's items are wholly inside ours",
        },
        {
          kind: "table",
          verdict: "ours-only",
          ours: [block("table", "table", "Hosting | 120\nTravel | 40", 1)],
          note: 'only ours: table "Hosting | 120 Travel | 40"',
        },
        // A form's line of dotted leaders, which the reference wrote as an item per
        // run of dots — and the rule under them as another — where our reading
        // produced no block at all. There is no text to compare and nothing to miss,
        // so the row *places* them: how many items it stands for, where they are in
        // points, and — as fractions of a US Letter page — the extent the page's own
        // shape is drawn with beside the note. One row for the line, not three.
        {
          kind: "text",
          verdict: "placed",
          theirs: [
            block("text", "text", ". . . . . . . . .", 1),
            block("text", "text", ". . . . . . .", 1),
            block("text", "text", "–", 1),
          ],
          note: "3 items with no letters in them along one line of page 1, box 50,560–550,581: the dots of a leader and the rule under them, placed as the one line they stand on rather than one row each",
          placement: {
            page: 1,
            width: 612,
            height: 792,
            x: 50 / 612,
            y: 560 / 792,
            w: 500 / 612,
            h: 21 / 792,
          },
        },
        // A second line of the form, on a page of its own: the page view is per page,
        // so the panel draws two pictures rather than one thumbnail per row.
        {
          kind: "text",
          verdict: "placed",
          theirs: [
            block("text", "text", ". . . . . .", 2),
            block("text", "text", ". . . . . . .", 2),
          ],
          note: "2 items with no letters in them along one line of page 2, box 60,120–540,140: the dots of a leader and the rule under them, placed as the one line they stand on rather than one row each",
          placement: {
            page: 2,
            width: 612,
            height: 792,
            x: 60 / 612,
            y: 120 / 792,
            w: 480 / 612,
            h: 20 / 792,
          },
        },
      ],
    };
  }

  function finish(id, tally, converted, failed, cancelled) {
    if (timer) clearInterval(timer);
    timer = null;
    emit("conversion-finished", {
      run: id,
      success: !cancelled && failed === 0,
      cancelled,
      exitCode: cancelled ? null : failed ? 1 : 0,
      tally: { converted, failed, total: tally },
    });
  }

  function start(input_path) {
    const id = ++run;
    stopped = false;
    let index = 0;
    let converted = 0;
    let failed = 0;

    // The backend announces a run before anything the CLI can have printed, and
    // the page keys its whole state off this event.
    emit("conversion-started", {
      run: id,
      command: `pdfextractor-cli --input ${input_path} --parallel 4 --recursive true --ocr auto --progress`,
    });
    // Exactly what the backend forwards: it strips terminal colour before the
    // page sees a log line, so the stand-in does not put escapes in either.
    emit("conversion-log", {
      run: id,
      line: " INFO pdfextractor_cli: 3 document(s) to convert, parallelism 4",
    });
    emit("conversion-plan", { run: id, total: DOCUMENTS.length, inputs: DOCUMENTS });

    timer = setInterval(() => {
      if (stopped) {
        finish(id, DOCUMENTS.length, converted, failed, true);
        return;
      }
      if (index >= DOCUMENTS.length) {
        finish(id, DOCUMENTS.length, converted, failed, false);
        return;
      }
      const input = DOCUMENTS[index];
      const ok = index !== 1; // the second document is the failure
      index += 1;
      if (ok) converted += 1;
      else failed += 1;
      emit("conversion-progress", {
        run: id,
        done: converted + failed,
        total: DOCUMENTS.length,
        input,
        ok,
        output: ok ? input.replace(/\.[^.]+$/, ".json") : undefined,
        error: ok ? undefined : "broken cross-reference table",
      });
      if (ok && comparing) emit("conversion-diff", diff(id, input));
    }, STEP_MS);

    return id;
  }

  window.__TAURI__ = {
    core: {
      async invoke(command, args) {
        if (command === "frontend_ready") return null;
        if (command === "pick_path") {
          if (args.kind === "folder") return "/golden/docling";
          return args.kind === "file" ? "/golden/docling/annual report.json" : "/docs";
        }
        if (command === "cancel_conversion") {
          stopped = true;
          return run;
        }
        if (command === "start_conversion") {
          comparing = Boolean(args.settings.reference);
          const id = start(args.settings.input);
          return {
            run: id,
            command: `pdfextractor-cli --input ${args.settings.input} --parallel 4 --recursive true --ocr auto --progress`,
          };
        }
        return null;
      },
    },
    event: {
      async listen(event, handler) {
        if (!listeners.has(event)) listeners.set(event, []);
        listeners.get(event).push(handler);
        return () => {};
      },
    },
  };
})();
