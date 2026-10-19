"""Exercise the real publisher/server and reload UI in Chromium (no mocked API)."""
import asyncio
import json
from pathlib import Path
import tempfile
import threading
from http.server import ThreadingHTTPServer
from playwright.async_api import async_playwright, expect
from test_viewer import viewer


async def check():
    with tempfile.TemporaryDirectory() as temporary:
        root = Path(temporary)
        site = root / 'site'
        (site / 'deck').mkdir(parents=True)
        output = root / 'review'
        slides = root / 'slides.json'
        ids = [f's{i}' for i in range(1, 31)]

        def build(ids, label='old', color='red'):
            slides.write_text(json.dumps({'title': 'Reload regression', 'slides': ids}))
            (site / 'figure.svg').write_text(f'<svg xmlns="http://www.w3.org/2000/svg" width="40" height="40"><rect width="40" height="40" fill="{color}"/></svg>')
            (site / 'style.css').write_text(f'body{{background:#1a1b26;color:{color};font:30px sans-serif}} section{{display:none}} section.visible{{display:block}}')
            markup = ''.join(f'<section><h1>{i} {label}</h1><img src="../figure.svg"></section>' for i in ids)
            (site / 'deck/index.html').write_text('<!doctype html><link rel="stylesheet" href="../style.css">' + markup + '''<script>
function show(){document.querySelectorAll('section').forEach((s,i)=>s.className=i+1===(parseInt(location.hash.slice(1))||1)?'visible':'');}
addEventListener('hashchange',show);show();</script>''')
            return viewer.publish(site, output, slides=slides)

        first = build(ids)
        server = ThreadingHTTPServer(('127.0.0.1', 0), viewer.handler_for(output))
        threading.Thread(target=server.serve_forever, daemon=True).start()
        try:
            async with async_playwright() as playwright:
                browser = await playwright.chromium.launch()
                page = await browser.new_page(viewport={'width': 1280, 'height': 800})
                errors = []
                page.on('pageerror', lambda e: errors.append(str(e)))
                await page.goto(f'http://127.0.0.1:{server.server_port}/#page=26&slide=s26')
                await expect(page.locator('#loaded')).to_have_text(first['revision'])
                frame = page.frame_locator('iframe:not(.loading)')
                await expect(frame.locator('section.visible')).to_contain_text('s26 old')
                second = build(['inserted', *ids], 'new', 'lime')
                await expect(page.locator('#notice')).to_contain_text('New version available', timeout=12000)
                # Update detection must not interrupt a review.
                await expect(frame.locator('section.visible')).to_contain_text('s26 old')
                await page.get_by_role('button', name='Reload slides').click()
                await expect(page.locator('#loaded')).to_have_text(second['revision'])
                await expect(frame.locator('section.visible')).to_contain_text('s26 new')
                assert await frame.locator('body').evaluate('(el)=>getComputedStyle(el).color') == 'rgb(0, 255, 0)'
                image_url = await frame.locator('section.visible img').get_attribute('src')
                assert image_url == '../figure.svg'
                image_src = await frame.locator('section.visible img').evaluate('(el)=>el.src')
                assert second['revision'] in image_src
                response = await page.request.get(image_src)
                assert 'lime' in await response.text()
                assert 'page=27' in page.url and 'slide=s26' in page.url
                third = build(['inserted', *ids], 'refreshed', 'cyan')
                await page.reload()
                await expect(page.locator('#loaded')).to_have_text(third['revision'])
                await expect(frame.locator('section.visible')).to_contain_text('s26 refreshed')
                deleted = build(['inserted', *[i for i in ids if i != 's26']], 'deleted', 'cyan')
                await page.get_by_role('button', name='Reload slides').click()
                await expect(page.locator('#loaded')).to_have_text(deleted['revision'])
                await expect(frame.locator('section.visible')).to_contain_text('s27 deleted')
                # A publication failure leaves the last completed pointer intact.
                (site / 'figure.svg').unlink()
                try:
                    viewer.publish(site, output, slides=slides)
                    raise AssertionError('Invalid publication accepted')
                except ValueError:
                    pass
                assert json.loads((output / 'latest.json').read_text())['revision'] == deleted['revision']
                failed = build(ids, 'unreachable', 'red')
                await page.route(f'**/revisions/{failed["revision"]}/deck/index.html', lambda route: route.fulfill(status=503, body='Unavailable'))
                await page.get_by_role('button', name='Reload slides').click()
                await expect(page.locator('#error')).to_contain_text('Reload failed')
                await expect(page.locator('#loaded')).to_have_text(deleted['revision'])
                await expect(frame.locator('section.visible')).to_contain_text('s27 deleted')
                await page.unroute_all()
                await page.route(f'**/revisions/{failed["revision"]}/figure.svg', lambda route: route.fulfill(status=503, body='Figure unavailable'))
                await page.get_by_role('button', name='Reload slides').click()
                await expect(page.locator('#error')).to_contain_text('Reload failed')
                await expect(page.locator('#loaded')).to_have_text(deleted['revision'])
                await expect(frame.locator('section.visible')).to_contain_text('s27 deleted')
                await page.unroute_all()
                await page.set_viewport_size({'width': 390, 'height': 844})
                await expect(page.get_by_role('button', name='Reload slides')).to_be_in_viewport()
                await page.get_by_role('button', name='Reload slides').click()
                await expect(page.locator('#loaded')).to_have_text(failed['revision'])
                await expect(frame.locator('section.visible')).to_contain_text('s27 unreachable')
                assert not errors, errors
                print('PASS: text/SVG/CSS freshness, update notice, insertion/deletion identity, browser refresh, failed publish/reload, mobile controls')
                await browser.close()
        finally:
            server.shutdown()
            server.server_close()


if __name__ == '__main__':
    asyncio.run(check())
