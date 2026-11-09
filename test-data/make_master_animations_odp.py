#!/usr/bin/env python3
"""Write `test-data/master_animations_impress.odp` with LibreOffice.

The fixture is a real Impress package with the two things the ODP reader has to
follow beyond a slide's own shape tree:

* a master page carrying a regular background text shape, which every slide
  that uses that master inherits unless it hides the background objects;
* slide animations, written by Impress' own animation model, so the exported
  `anim:` tree and its `smil:targetElement` references are a producer's.

The deck has two slides. The first uses the master page with its background
shape and animates two of its own shapes; the second uses the same master page
with background objects turned off. `crates/cli/tests/odp_test.rs` reads it.

    python3 test-data/make_master_animations_odp.py

Run from the repository root. System Python must be able to `import uno`, and
LibreOffice's `soffice` must be on PATH.
"""

import pathlib
import shutil
import socket
import subprocess
import tempfile
import time

import uno
from com.sun.star.awt import Point, Size
from com.sun.star.beans import PropertyValue

ROOT = pathlib.Path(__file__).resolve().parent
TARGET = ROOT / "master_animations_impress.odp"
PORT = 2012


def connect(profile: pathlib.Path):
    """Start a private headless LibreOffice and connect to it over UNO."""
    soffice = shutil.which("soffice")
    if soffice is None:
        raise SystemExit("soffice is not on PATH")
    process = subprocess.Popen(
        [
            soffice,
            "--headless",
            "--invisible",
            "--norestore",
            "--nologo",
            f"-env:UserInstallation={profile.as_uri()}",
            f"--accept=socket,host=127.0.0.1,port={PORT};urp;",
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        try:
            with socket.create_connection(("127.0.0.1", PORT), timeout=1):
                break
        except OSError:
            if process.poll() is not None:
                raise SystemExit("LibreOffice exited before it accepted a connection")
            time.sleep(0.25)
    else:
        process.terminate()
        raise SystemExit("LibreOffice did not accept a connection")

    local = uno.getComponentContext()
    resolver = local.ServiceManager.createInstanceWithContext(
        "com.sun.star.bridge.UnoUrlResolver", local
    )
    context = resolver.resolve(
        f"uno:socket,host=127.0.0.1,port={PORT};urp;StarOffice.ComponentContext"
    )
    return process, context


def text_shape(document, name: str, x: int, y: int, width: int, height: int):
    shape = document.createInstance("com.sun.star.drawing.TextShape")
    shape.Name = name
    shape.Position = Point(x, y)
    shape.Size = Size(width, height)
    return shape


def animation_node(context, service: str):
    return context.ServiceManager.createInstanceWithContext(
        f"com.sun.star.animations.{service}", context
    )


def add_effect(context, sequence, target, begin: float):
    """One entrance effect, in the parallel container its delay belongs to."""
    effect = animation_node(context, "AnimateSet")
    effect.Target = target
    effect.AttributeName = "visibility"
    effect.To = "visible"
    effect.Begin = 0.0
    effect.Duration = 0.5

    node = animation_node(context, "ParallelTimeContainer")
    node.Begin = begin
    node.appendChild(effect)
    sequence.appendChild(node)


def main() -> None:
    with tempfile.TemporaryDirectory() as scratch:
        profile = pathlib.Path(scratch) / "profile"
        process, context = connect(profile)
        try:
            desktop = context.ServiceManager.createInstanceWithContext(
                "com.sun.star.frame.Desktop", context
            )
            document = desktop.loadComponentFromURL(
                "private:factory/simpress", "_blank", 0, ()
            )

            first = document.getDrawPages().getByIndex(0)
            master = first.getMasterPage()
            background = text_shape(document, "Confidential", 1500, 17600, 24000, 1500)
            master.add(background)
            background.setString("Confidential \u2014 internal use only")

            title = text_shape(document, "Title", 1500, 1200, 24000, 2500)
            first.add(title)
            title.setString("Animated background")
            body = text_shape(document, "Body", 1500, 4500, 24000, 2500)
            first.add(body)
            body.setString("This line fades in.")

            # The slides' animations live in the main sequence Impress already
            # made for the timing root; each effect waits for the one before
            # it, so `begin` is its delay.
            root = first.getAnimationNode()
            sequence = next(iter(root.createEnumeration()), None)
            if sequence is None:
                sequence = animation_node(context, "SequenceTimeContainer")
                root.appendChild(sequence)
            add_effect(context, sequence, body, 0.0)
            add_effect(context, sequence, title, 0.5)

            second = document.getDrawPages().insertNewByIndex(1)
            second.setMasterPage(master)
            try:
                second.setPropertyValue("BackgroundObjectsVisible", False)
            except Exception:
                second.setPropertyValue("IsBackgroundObjectsVisible", False)
            suppressed = text_shape(document, "Suppressed", 1500, 1200, 24000, 2500)
            second.add(suppressed)
            suppressed.setString("Background objects are hidden on this slide.")

            target = TARGET.resolve()
            target.unlink(missing_ok=True)
            document.storeAsURL(
                target.as_uri(),
                (PropertyValue(Name="FilterName", Value="impress8"),),
            )
            document.close(True)
        finally:
            process.terminate()
            process.wait(timeout=10)
    print(f"wrote {TARGET}")


if __name__ == "__main__":
    main()
