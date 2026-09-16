"""Native private-Sway background-image lifecycle and sampled-pixel evidence."""
import json
import math
from pathlib import Path

from scripts.gui_tab_placement import configuration_for_position, native_keyboard_lifetime
from scripts.gui_wayland_capture import capture_private_output


def pixel_probes(source, baseline, viewport, fit, opacity):
    """Independent inverse fit at clear content points, outside the toast area."""
    left, top, width, height = viewport
    sx, sy = width / source.width, height / source.height
    if fit == "cover":
        sx = sy = max(sx, sy)
    elif fit == "contain":
        sx = sy = min(sx, sy)
    elif fit == "center":
        sx = sy = 1
    elif fit != "stretch":
        raise ValueError("invalid background fit")
    probes = []
    for fx in (0.1, 0.3, 0.45, 0.7, 0.9):
        for fy in (0.38, 0.56, 0.76, 0.93):
            x, y = int(left + width * fx), int(top + height * fy)
            if x > (left + width) / 2 and y > (top + height) * 0.65:
                continue  # transient configuration toasts, not image content
            u = (x + 0.5 - left - width / 2) / sx + source.width / 2
            v = (y + 0.5 - top - height / 2) / sy + source.height / 2
            expected = baseline.getpixel((x, y))
            if opacity and 0 <= u < source.width and 0 <= v < source.height:
                # Stay clear of the patterned fixture's color discontinuities,
                # where independent JPEG decoders/filter precision may differ.
                if min(u % 160, 160 - u % 160, v % 120, 120 - v % 120) < 3:
                    continue
                expected = source.getpixel((int(u), int(v)))
            probes.append(((x, y), expected))
    if len(probes) < 8:
        raise ValueError("background fixture has too few independent probes")
    return probes


def probes_match(image, probes):
    return all(max(abs(a - b) for a, b in zip(image.getpixel(point), color)) <= 5
               for point, color in probes)


def run_background_scenario(gui, binary: Path, run_dir: Path, env: dict):
    run_dir.mkdir(parents=True, exist_ok=True)
    transcript = run_dir / "pty-transcript.jsonl"
    program, arguments = gui.fixture_command(transcript, instance=175)
    arguments.append("--background-fixture")
    app = gui.AppProcess(binary, run_dir, "wayland", program, arguments, env)
    config_path = Path(app.env["XDG_CONFIG_HOME"]) / "termeric/config.toml"
    base_config = config_path.read_text()
    records = []
    size = (960, 720)
    position = "top"
    search = False

    def starts():
        return [int(row["pid"]) for row in gui.read_records(transcript) if row.get("event") == "started"]

    def viewport():
        rows, cols, width, height = gui.latest_terminal_geometry(gui.read_records(transcript), starts()[0])
        cell_height = height / rows
        toolbar = max(18, math.floor(cell_height * 1.1 + 0.5)) + 10
        left = 220 if position == "left" else 0
        top = toolbar + (cell_height + 16 if search else 0)
        return left, top, size[0] - left, size[1] - top

    def configure(path=None, fit="cover", opacity=1):
        before = app.log_count("Config hot-reloaded")
        content = configuration_for_position(base_config, position)
        if path is not None:
            content += f'\n[window.background_image]\npath = {gui.toml_string(str(path))}\nfit = "{fit}"\nopacity = {float(opacity)}\n'
        temporary = config_path.with_suffix(".new")
        temporary.write_text(content)
        temporary.replace(config_path)
        gui.wait_until("background configuration reload", lambda: app.log_count("Config hot-reloaded") > before)

    def capture(name, probes=None):
        latest = None
        def ready():
            nonlocal latest
            latest = capture_private_output(app.env)
            return latest.size == size and (probes is None or probes_match(latest, probes))
        try:
            gui.wait_until(f"background pixels {name}", ready, 12)
        finally:
            if latest is not None:
                latest.save(run_dir / f"{name}.png")
        records.append({"name": name, "size": list(size), "viewport": list(viewport()),
                        "samples": len(probes or [])})
        return latest

    def source_fixture(path):
        image = gui.Image.new("RGB", (480, 240))
        for x in range(480):
            for y in range(240):
                colors = ((220, 40, 20), (30, 190, 50), (30, 60, 220))
                color = colors[x // 160]
                if y >= 120:
                    color = (color[1], color[2], color[0])
                image.putpixel((x, y), color)
        image.save(path)
        with gui.Image.open(path) as decoded:
            return decoded.convert("RGB")

    try:
        # Defer requested fault injection until an image is actually installed.
        triggers, app.renderer_triggers = app.renderer_triggers, {}
        try:
            app.wait_ready("wayland")
        finally:
            app.renderer_triggers = triggers
        gui.wait_until("background fixture PTY", lambda: len(starts()) == 1)
        original = starts()[0]
        app.renderer_resize("wayland", *size)
        gui.wait_until("background fixture geometry", lambda: gui.latest_terminal_geometry(
            gui.read_records(transcript), original)[2] < size[0])
        # Keep a keyboard alive for input-dependent phases of the fixture.
        with native_keyboard_lifetime(gui, app, "wayland", duration_ms=180000):
            for position in ("top", "left"):
                configure()
                gui.time.sleep(0.15)
                baseline = capture(f"{position}-disabled")
                left, top, _, _ = viewport()
                rows, cols, width, height = gui.latest_terminal_geometry(gui.read_records(transcript), original)
                foreground = (int(left + 8 + 10 * width / cols), int(top + 8 + 4.5 * height / rows))
                if not probes_match(baseline, [(foreground, (255, 0, 255))]):
                    raise gui.AcceptanceError("native fixture lacks its explicit cell-background marker")
                for extension in ("png", "jpg"):
                    path = run_dir / f"background.{extension}"
                    source = source_fixture(path)
                    for fit in ("cover", "contain", "stretch", "center"):
                        for opacity in (0, 1):
                            configure(path, fit, opacity)
                            probes = pixel_probes(source, baseline, viewport(), fit, opacity)
                            probes.append((foreground, (255, 0, 255)))
                            capture(f"{position}-{extension}-{fit}-{opacity}", probes)
            # Use a stable source and baseline for lifecycle checks in the rail.
            configure()
            baseline = capture("lifecycle-disabled")
            path = run_dir / "background.png"
            source = source_fixture(path)
            configure(path, "stretch")
            capture("lifecycle-loaded", pixel_probes(source, baseline, viewport(), "stretch", 1))
            # Replace atomically with another image; the loaded pixels, not
            # just a completion log, must change.
            replacement = run_dir / "replacement.png"
            gui.Image.new("RGB", (480, 240), (180, 100, 20)).save(replacement)
            replacement.replace(path)
            source = gui.Image.new("RGB", (480, 240), (180, 100, 20))
            probes = pixel_probes(source, baseline, viewport(), "stretch", 1)
            capture("replaced", probes)
            before = app.log_count("Background image load failed")
            path.write_bytes(b"not a PNG or JPEG")
            gui.wait_until("malformed background refusal", lambda: app.log_count("Background image load failed") > before)
            capture("malformed-keeps-last-good", probes)
            before = app.log_count("Background image loaded:")
            source = source_fixture(path)
            gui.wait_until("restored background source", lambda: app.log_count("Background image loaded:") > before)
            capture("restored-pattern", pixel_probes(source, baseline, viewport(), "stretch", 1))
            # Native search geometry and real split PTYs while the image remains
            # one shared window background rather than duplicated per pane.
            app.wayland_chord(("ctrl", "shift"), "f")
            search = True
            capture("search-open", pixel_probes(source, baseline, viewport(), "stretch", 1))
            app.wayland_chord((), "Escape")
            search = False
            app.wayland_chord(("alt", "shift"), "Right")
            gui.wait_until("background split PTY", lambda: len(starts()) == 2)
            capture("split", pixel_probes(source, baseline, viewport(), "stretch", 1))
            size = (800, 600)
            app.renderer_resize("wayland", *size)
            capture("resized", pixel_probes(source, baseline, viewport(), "stretch", 1))
            size = (960, 720)
            app.renderer_resize("wayland", *size)
            capture("resize-restored", pixel_probes(source, baseline, viewport(), "stretch", 1))
            app.wait_ready("wayland")  # exercise each requested renderer fault
            app.renderer_resize("wayland", *size)
            capture("after-renderer-recovery", pixel_probes(source, baseline, viewport(), "stretch", 1))
            configure()
            # Disabling must actually remove the image; compare clear points
            # with the original no-image baseline, excluding the new divider.
            capture("disabled-after-recovery", pixel_probes(source, baseline, viewport(), "stretch", 0))
        app.close_normally("wayland")
        gui.wait_until("background fixture cleanup", lambda: all(not Path(f"/proc/{pid}").exists() for pid in starts()))
        gui.assert_no_runtime_errors(app)
        (run_dir / "background.json").write_text(json.dumps({"schema": 1, "backend": "private-sway",
            "frames": records, "pids": starts(), "renderer_injections": sorted(triggers),
            "coverage": "sampled native PNG/JPEG fit/opacity, cell-background layering, rail/search/split, replacement/refusal/disable and requested simulated recovery; not X11, plugin chrome, subjective contrast or physical GPU recovery"}, indent=2) + "\n")
    finally:
        app.stop_after_failure()
