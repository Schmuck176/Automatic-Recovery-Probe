// Drives the Automatic tab in a real browser (Chromium, through Playwright),
// the way a person would: Get ready, the self-test, the folders, Start, then
// Finalise, against the program started with a test disk image. Takes
// screenshots of each part.
//
//   node tools/auto_ui_check.js <automatic-recovery-probe> <work folder> <test disk image> <mapfile>
const { spawn } = require("child_process");
const fs = require("fs");
const path = require("path");
let chromium;
for (const p of ["playwright", "/opt/npm-tools/node_modules/playwright", "/home/claude/.npm-global/lib/node_modules/@playwright/mcp/node_modules/playwright"]) {
  try { ({ chromium } = require(p)); break; } catch (e) { /* next */ }
}
const [exe, work, disk, mapfile] = process.argv.slice(2);
const home = path.join(work, "home");
fs.rmSync(work, { recursive: true, force: true });
fs.mkdirSync(home, { recursive: true });
fs.copyFileSync(mapfile, path.join(home, "second attempt.log"));
const results = [];
const check = (ok, what) => { results.push([!!ok, what]); console.log((ok ? "ok   " : "FAIL ") + what); };
// A port nothing else uses, picked by the system (a fixed range can fall in
// one Windows keeps for itself: "access forbidden", os error 10013).
function freePort() {
  return new Promise((resolve, reject) => {
    const s = require("net").createServer();
    s.on("error", reject);
    s.listen(0, "127.0.0.1", () => { const p = s.address().port; s.close(() => resolve(p)); });
  });
}
let port;
const key = "autouikey0123456789abcdef";
// Icons shown with no words beside them (in their own element or the one
// around it), which the page must never have: a picture alone says nothing
// sure. (Hidden ones don't count.)
function iconsWithoutWords(page) {
  return page.evaluate(() => {
    const bad = [];
    for (const svg of document.querySelectorAll("svg.ic")) {
      if (!svg.getClientRects().length) continue;
      const near = [svg.parentElement, svg.parentElement && svg.parentElement.parentElement];
      if (!near.some((el) => el && el.textContent.trim())) {
        bad.push((svg.querySelector("use") || { getAttribute: () => "?" }).getAttribute("href") + " in ." + svg.parentElement.className);
      }
    }
    return bad;
  });
}

(async () => {
  port = await freePort();
  const server = spawn(exe, ["ui", "--port", String(port), "--token", key, "--no-browser", "--test-disk", disk, "--home", home],
    { stdio: ["ignore", "pipe", "pipe"] });
  let out = "";
  const log = fs.createWriteStream(path.join(work, "server-output.txt"));
  server.stdout.on("data", (d) => { out += d; log.write(d); });
  server.stderr.on("data", (d) => { log.write(d); });
  for (let i = 0; i < 50 && !out.includes("http://"); i++) await new Promise((r) => setTimeout(r, 100));
  const url = `http://127.0.0.1:${port}/${key}/`;
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1180, height: 900 } });
  const errors = [];
  page.on("pageerror", (e) => errors.push(String(e)));
  page.on("console", (m) => { if (m.type() === "error") errors.push(m.text()); });
  const shot = (name, full) => page.screenshot({ path: path.join(work, name + ".png"), fullPage: !!full });
  const text = (sel) => page.locator(sel).innerText();
  try {
    await page.goto(url);
    await page.waitForSelector("#b-ready");
    check(await page.locator("#view-auto").isVisible(), "the page opens on the Automatic tab");
    check(!(await page.locator("#view-steps").isVisible()), "the steps are in their own tab");
    // 1. Get ready.
    await page.click("#b-ready");
    await page.waitForFunction(() => document.querySelectorAll("#ar-ready .checks li").length > 3, null, { timeout: 30000 });
    const ready = await text("#ar-ready");
    check(/mapfile/i.test(ready) && /second attempt\.log/.test(ready), "Get ready finds the mapfile");
    check(/FFmpeg/.test(ready), "Get ready lists the tools");
    await page.click("#b-ready-test");
    await page.waitForFunction(() => /Every check passed/.test(document.getElementById("ar-ready").innerText), null, { timeout: 120000 });
    await page.waitForFunction(() => /Ready/.test(document.getElementById("ast-ready").textContent), null, { timeout: 30000 });
    check(true, "the self-test passes and Get ready says Ready");
    await shot("auto-1-ready");
    // 3. The folders.
    await page.fill("#af-image", path.join(work, "IMAGE"));
    await page.dispatchEvent("#af-image", "change");
    await page.fill("#af-work", path.join(work, "SPARE", "WORK"));
    await page.dispatchEvent("#af-work", "change");
    await page.fill("#af-copy2", path.join(work, "COPY 2"));
    await page.dispatchEvent("#af-copy2", "change");
    await page.waitForFunction(() => /Chosen/.test(document.getElementById("ast-folders").textContent), null, { timeout: 10000 });
    check(true, "the folders are chosen");
    // 4. Start.
    await page.click("#b-auto-start");
    await page.waitForFunction(() => /Running|Waiting/.test(document.getElementById("ast-start").textContent), null, { timeout: 10000 })
      .then(() => check(true, "Start runs"), () => check(false, "Start runs"));
    await page.waitForTimeout(1500);
    await shot("auto-2-running");
    await page.waitForFunction(() => /Done|Needs attention/.test(document.getElementById("ast-start").textContent), null, { timeout: 600000 });
    const st = await text("#ar-start");
    check(/Start is done/.test(st), "Start finishes: " + (st.match(/Start (is done|stopped)[^\n]*/) || [""])[0]);
    const done = await page.locator("#ar-start ul.stages li.done").count();
    const skipped = await page.locator("#ar-start ul.stages li.skipped").count();
    check(done + skipped === 15, `every stage is done or skipped (${done} done, ${skipped} skipped)`);
    await shot("auto-3-done", true);
    // The steps tab shows what Start did.
    await page.click("#tab-steps");
    check(/Done/.test(await text("#st-check")) && /Done/.test(await text("#st-copy")), "the steps tab shows the map check and the copying done");
    await page.click("#tab-auto");
    // 5. What is left.
    const left = await text("#ar-left");
    check(/repaired and put back|Nothing is left|hand-off/i.test(left), "What is left shows the repairs");
    // 6. Finalise.
    await page.click("#b-finalise");
    await page.waitForFunction(() => /Done|Needs attention/.test(document.getElementById("ast-final").textContent), null, { timeout: 300000 });
    check(/Finalised/.test(await text("#ar-final")), "Finalise finishes");
    await shot("auto-4-finalised", true);
    // Every icon on the page comes with words saying what it is for.
    check((await page.locator("#rail .rail-item").count()) === 6, "the list on the left names the six parts");
    const mute = await iconsWithoutWords(page);
    check(mute.length === 0, "every icon has words beside it" + (mute.length ? ": " + mute.slice(0, 5).join(" | ") : ""));
    // The parts stay where they belong as the window changes size. At the
    // PC's own (a 27-inch 4K screen at 150%: 2560 by about 1350): where you
    // are, the guide and what is happening now, side by side.
    const box = (sel) => page.locator(sel).boundingBox();
    const sideways = () => page.evaluate(() => document.documentElement.scrollWidth - document.documentElement.clientWidth);
    await page.setViewportSize({ width: 2560, height: 1352 });
    await page.waitForTimeout(300);
    let [r, m, n] = [await box("#rail"), await box("#main"), await box("#now")];
    check(r && m && n && r.x + r.width <= m.x && m.x + m.width <= n.x && Math.abs(r.y - n.y) < 40 && (await sideways()) <= 1,
      "at 2560 px: where you are, the guide and Now side by side");
    await shot("auto-6-wide");
    // Half the screen: Now goes under where you are.
    await page.setViewportSize({ width: 1265, height: 1352 });
    await page.waitForTimeout(300);
    [r, m, n] = [await box("#rail"), await box("#main"), await box("#now")];
    check(r && m && n && n.y >= r.y + r.height - 1 && n.x + n.width <= m.x && (await sideways()) <= 1,
      "at half the screen: Now under where you are, the guide beside them");
    // Narrower: where you are on top, Now only while something runs.
    await page.setViewportSize({ width: 900, height: 1000 });
    await page.waitForTimeout(300);
    [r, m, n] = [await box("#rail"), await box("#main"), await box("#now")];
    check(r && m && r.y + r.height <= m.y && !n && (await sideways()) <= 1, "at 900 px: where you are on top, and nothing running in the way");
    // Narrow window (a phone-sized one): nothing sticks out sideways.
    await page.setViewportSize({ width: 390, height: 844 });
    await page.waitForTimeout(300);
    const wide = await sideways();
    check(wide <= 1, "nothing sticks out of a narrow window (" + wide + " px)");
    await shot("auto-5-narrow");
  } catch (e) {
    check(false, "the test ran to the end: " + e.message);
    await shot("auto-error", true).catch(() => {});
  }
  check(errors.length === 0, "no errors in the page" + (errors.length ? ": " + errors.slice(0, 3).join(" | ") : ""));
  await browser.close();
  server.kill();
  const passed = results.filter((r) => r[0]).length;
  console.log(`\n${passed} of ${results.length} checks passed`);
  process.exit(passed === results.length ? 0 : 1);
})();
