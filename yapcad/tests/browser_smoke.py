"""Pane smoke test against a running server for examples/bracket:

bin/yapcad serve --cwd examples/bracket --port 49131 &
uv run python tests/browser_smoke.py [http://localhost:49131]
"""

import asyncio
import sys
from pathlib import Path

from playwright.async_api import async_playwright

URL = sys.argv[1] if len(sys.argv) > 1 else "http://localhost:49131"
LIT = """() => {
  const c = document.getElementById('canvas');
  const g = c.getContext('webgl2') || c.getContext('webgl');
  const px = new Uint8Array(4 * c.width * c.height);
  g.readPixels(0, 0, c.width, c.height, g.RGBA, g.UNSIGNED_BYTE, px);
  let lit = 0;
  for (let i = 0; i < px.length; i += 4) if (px[i + 2] > 120 && px[i + 2] > px[i] + 30) lit++;
  return lit;
}"""


async def built(page, action, condition="true", timeout=60000):
    """Perform action and wait for a new, finished run satisfying condition."""
    previous = await page.evaluate("workbench.run?.id ?? ''")
    await action()
    await page.wait_for_function(
        f"workbench.run && workbench.run.id !== '{previous}' && workbench.run.status !== 'running' && workbench.view !== undefined && ({condition})",
        timeout=timeout,
    )
    await page.wait_for_function("document.querySelector('#stop').disabled")
    return await page.evaluate("workbench.run")


async def main():
    output = Path("/tmp/yapcad-plugin-smoke")
    output.mkdir(exist_ok=True)
    async with async_playwright() as playwright:
        browser = await playwright.chromium.launch(
            args=["--use-gl=angle", "--use-angle=swiftshader"]
        )
        page = await browser.new_page(viewport={"width": 1440, "height": 900})
        errors = []
        page.on("pageerror", lambda e: errors.append(str(e)))
        page.on("console", lambda m: m.type == "error" and errors.append(m.text))
        await page.goto(URL)
        await page.wait_for_function(
            "document.querySelectorAll('#command option').length === 3"
        )
        await page.locator("#live").uncheck()
        await page.select_option("#command", "PLATE")
        await page.locator("#reset").click()
        await page.locator("#live").check()
        run = await built(
            page, page.locator("#build").click, "workbench.view?.kind === 'solid'"
        )
        assert run["status"] == "ok" and run["params"] == {}, run
        assert await page.evaluate(LIT) > 2000, "model not visible"

        # Live rebuild on a parameter change.
        width = page.locator("#params input[name=width][type=number]")
        await width.fill("90")
        run = await built(
            page,
            lambda: width.dispatch_event("change"),
            "workbench.run.params.width === 90",
        )
        size = run["stats"]["bbox"][1][0] - run["stats"]["bbox"][0][0]
        assert run["status"] == "ok" and abs(size - 90) < 1e-6, size

        # Pin a point on the model and annotate the view.
        await page.select_option("#view", "top")
        await page.select_option("#mode", "pin")
        box = await page.locator("#canvas").bounding_box()
        await page.mouse.click(
            box["x"] + box["width"] * 0.62, box["y"] + box["height"] * 0.5
        )
        try:
            await page.wait_for_function("workbench.pins.length === 1", timeout=5000)
        except Exception:
            await page.screenshot(path=str(output / "fail.png"))
            raise
        await page.screenshot(path=str(output / "model.png"))
        payloads = []

        async def capture(route):
            payloads.append(route.request.post_data_json)
            await route.fulfill(
                status=200, content_type="application/json", body='{"ok":true}'
            )

        await page.route("**/__portal/edit-stack", capture)
        await page.locator("#annotate").click()
        await page.locator("#body").fill("Move this bolt hole 5 mm outward.")
        await page.locator("#send").click()
        await page.wait_for_function("!document.getElementById('note').open")
        context = payloads[0]["items"][0]["context"]
        assert context["plugin"] == "yapcad" and context["pins"][0]["part"], context
        assert context["params"]["width"] == 90
        assert payloads[0]["items"][0]["image"]["dataUrl"].startswith(
            "data:image/png;base64,"
        )

        # 2D results, scalar results, source and reference views.
        await page.locator("#live").uncheck()
        await page.select_option("#command", "PROFILE")
        run = await built(
            page, page.locator("#build").click, "workbench.view?.kind === '2d'"
        )
        assert run["command"] == "PROFILE", run
        await page.screenshot(path=str(output / "profile.png"))
        await page.select_option("#command", "MASS_G")
        run = await built(
            page, page.locator("#build").click, "workbench.view?.kind === 'value'"
        )
        assert run["value"] == 29.16, run
        assert "29.16" in await page.locator("#overlay").inner_text()
        await page.select_option("#tab", "source")
        await page.wait_for_function(
            "document.querySelectorAll('#source div.command').length === 3"
        )
        await page.select_option("#tab", "reference")
        await page.locator("#ref-search").fill("cylinder")
        await page.wait_for_function(
            "document.querySelector('#ref-list').textContent.includes('cylinder(')"
        )
        await page.screenshot(path=str(output / "reference.png"))
        await page.set_viewport_size({"width": 390, "height": 844})
        await page.select_option("#tab", "model")
        await page.screenshot(path=str(output / "mobile.png"))
        assert not errors, errors
        await browser.close()
    print(f"ok; screenshots in {output}")


asyncio.run(main())
