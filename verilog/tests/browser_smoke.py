"""Run against the counter preview: uv run python tests/browser_smoke.py."""

import asyncio
from pathlib import Path

from playwright.async_api import async_playwright


async def main():
    output = Path("/tmp/verilog-plugin-smoke")
    output.mkdir(exist_ok=True)
    async with async_playwright() as playwright:
        browser = await playwright.chromium.launch()
        page = await browser.new_page(viewport={"width": 1440, "height": 900})
        errors = []
        page.on("pageerror", lambda e: errors.append(str(e)))
        await page.goto("http://localhost:49120")
        await page.wait_for_function("state.traces.length > 0")
        assert await page.evaluate("state.meta.end") == 500000
        colors = await page.evaluate("""() => {
            const data=ctx.getImageData(0,0,canvas.width,canvas.height).data;
            let green=0;for(let i=0;i<data.length;i+=4)if(data[i+1]>150&&data[i]<180&&data[i+2]<150)green++;
            return green;
        }""")
        assert colors > 100, colors
        await page.locator("#zoom-in").click()
        await page.wait_for_function("state.end-state.start < 500000")
        await page.wait_for_function(
            "localStorage.getItem(key()) && JSON.parse(localStorage.getItem(key())).end-JSON.parse(localStorage.getItem(key())).start < 500000"
        )
        before = await page.evaluate("[state.start,state.end]")
        await page.reload()
        await page.wait_for_function("state.traces.length > 0")
        assert await page.evaluate("[state.start,state.end]") == before
        await page.locator("#fit").click()
        await page.wait_for_function("state.end === 500000")
        await page.screenshot(path=str(output / "desktop.png"))
        await page.locator("#annotate").click()
        await page.locator("#body").fill(
            "Check the reset release and counter increment."
        )
        payloads = []

        async def capture(route):
            payloads.append(route.request.post_data_json)
            await route.fulfill(
                status=200, content_type="application/json", body='{"ok":true}'
            )

        await page.route("**/__portal/edit-stack", capture)
        await page.locator("#send").click()
        await page.wait_for_function("!document.getElementById('note').open")
        assert payloads[0]["items"][0]["context"]["signals"]
        assert payloads[0]["items"][0]["image"]["dataUrl"].startswith(
            "data:image/png;base64,"
        )
        await page.unroute("**/__portal/edit-stack")
        await page.locator("#annotate").click()
        await page.locator("#body").fill("Keep this note on submission failure")
        await page.locator("#send").click()
        await page.wait_for_function(
            "document.getElementById('note-status').textContent.includes('Not queued')"
        )
        assert (
            await page.locator("#body").input_value()
            == "Keep this note on submission failure"
        )
        await page.locator("button[value=cancel]").click()
        # Exercise the UI-triggered run, completion invalidation, and retained view.
        run_id = await page.evaluate("state.run.id")
        await page.locator("#run").click()
        await page.wait_for_function(
            "old => state.run.id !== old && state.run.status === 'passed'", arg=run_id
        )
        await page.wait_for_function("state.traces.length > 0")
        for viewport in [{"width": 390, "height": 844}, {"width": 1024, "height": 768}]:
            await page.set_viewport_size(viewport)
            await page.wait_for_function(
                "canvas.clientWidth === document.getElementById('waves').clientWidth"
            )
            assert await page.evaluate(
                "document.documentElement.scrollWidth <= innerWidth"
            )
            await page.screenshot(path=str(output / f"view-{viewport['width']}.png"))
        # Two pointers -> one pointer -> none must leave no latched gesture.
        await page.evaluate("""() => {
          const box=canvas.getBoundingClientRect();
          const send=(type,id,x)=>canvas.dispatchEvent(new PointerEvent(type,{pointerId:id,clientX:box.left+x,clientY:box.top+50,bubbles:true,pointerType:'touch'}));
          const original=canvas.setPointerCapture;canvas.setPointerCapture=()=>{};
          send('pointerdown',1,250);send('pointerdown',2,400);send('pointermove',2,450);
          send('pointerup',2,450);send('pointermove',1,270);send('pointerup',1,270);
          canvas.setPointerCapture=original;
        }""")
        assert await page.evaluate(
            "state.pointers.size === 0 && state.gesture === null"
        )
        assert not errors, errors
        print(
            f"PASS: render, zoom persistence, run, annotation payload/failure, mobile layout, pointer reset. Screenshots: {output}"
        )
        await browser.close()


asyncio.run(main())
