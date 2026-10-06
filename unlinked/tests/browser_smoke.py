"""Browser check against the examples: start `bin/unlinked serve --cwd examples --port 49140`,
then `uv run python tests/browser_smoke.py`."""

import asyncio
from pathlib import Path

from playwright.async_api import async_playwright


async def main():
    output = Path("/tmp/unlinked-plugin-smoke")
    output.mkdir(exist_ok=True)
    async with async_playwright() as p:
        browser = await p.chromium.launch()
        page = await browser.new_page(viewport={"width": 1440, "height": 900})
        errors = []
        page.on("pageerror", lambda e: errors.append(str(e)))
        await page.goto("http://127.0.0.1:49140")
        await page.wait_for_selector("#model option", state="attached")
        await page.select_option("#model", "cruise_control_pi.mdl")
        await page.wait_for_selector("#svg-host svg .block")
        assert await page.locator("#svg-host svg .block").count() == 12
        await page.screenshot(path=str(output / "diagram.png"))
        await page.select_option("#model", "nested.mdl")
        await page.wait_for_selector("#system:not([hidden])")
        await page.select_option("#system", "nested/Outer//Slash/Inner")
        await page.wait_for_function(
            "document.querySelectorAll('#svg-host svg .block').length > 0"
        )
        await page.select_option("#model", "cruise_control_pi.mdl")
        await page.wait_for_selector("#svg-host svg .block")
        await page.select_option("#view", "simulate")
        assert await page.input_value("#stop") == "30"
        await page.fill("#stop", "5")
        await page.click("#run")
        await page.wait_for_selector("#legend li")
        # 12 blocks plus the transfer function's internal states.
        assert await page.locator("#legend li").count() >= 12
        assert "Set speed (1)" in await page.locator("#legend").inner_text()
        painted = await page.evaluate("""() => {
            const c = document.getElementById('plot'), d = c.getContext('2d').getImageData(0,0,c.width,c.height).data;
            let n = 0; for (let i = 0; i < d.length; i += 4) if (d[i] > 100 && d[i+2] > 200) n++; return n;
        }""")
        assert painted > 200, painted
        await page.screenshot(path=str(output / "simulate.png"))
        await page.set_viewport_size({"width": 390, "height": 844})
        await page.wait_for_timeout(200)
        assert await page.evaluate("document.documentElement.scrollWidth <= innerWidth")
        await page.screenshot(path=str(output / "mobile.png"))
        assert not errors, errors
        await browser.close()


asyncio.run(main())
