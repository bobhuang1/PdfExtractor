#![no_main]

use libfuzzer_sys::fuzz_target;
use pdfextractor_fuzz::{limits, note};

fuzz_target!(|data: &[u8]| {
    // XML is reached inside a package, but the depth and DOCTYPE rules are the
    // same for every caller. This target exercises them directly, including the
    // size ceiling and the event-loop deadline.
    let checked = limits().check_xml(data);
    note("xml", &checked);
});
