"""Browser check against the counter example: start `bin/visilog serve --cwd examples/counter`
on port 49130, then `uv run python tests/browser_smoke.py`."""

import asyncio
from pathlib import Path

from playwright.async_api import async_playwright


async def main():
    output = Path("/tmp/visilog-plugin-smoke")
    output.mkdir(exist_ok=True)
    async with async_playwright() as p:
        browser = await p.chromium.launch()
        page = await browser.new_page()
        errors = []
        page.on("pageerror", lambda e: errors.append(str(e)))
        await page.goto("http://127.0.0.1:49130")
        await page.wait_for_selector("#test option", state="attached")
        frame = page.frame_locator("#frame")
        await frame.locator("#viewport text").first.wait_for()
        before = await frame.locator("#time").inner_text()
        await frame.locator("#step").click()
        await page.wait_for_timeout(500)
        await frame.locator("#step").click()
        await page.wait_for_timeout(500)
        after = await frame.locator("#time").inner_text()
        assert before != after, (before, after)
        for width, height in [(1440, 900), (390, 844), (1024, 768)]:
            await page.set_viewport_size({"width": width, "height": height})
            await frame.locator("#fit").click()
            await page.wait_for_timeout(150)
            await page.screenshot(path=str(output / f"{width}.png"))
            assert await frame.locator("#viewport text").count() > 2
            assert await page.evaluate(
                "document.documentElement.scrollWidth <= innerWidth"
            )
        await page.click("#load")
        await frame.locator("#viewport text").first.wait_for()
        assert not errors, errors
        await browser.close()


asyncio.run(main())
