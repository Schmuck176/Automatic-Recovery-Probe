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
    // What each folder needs, beside it: worked out from the mapfile.
    await page.evaluate(() => loadPlaces());
    const needs = await page.waitForFunction(() => /Needs/.test(document.getElementById("facts-image").textContent) &&
      /Needs up to/.test(document.getElementById("facts-work").textContent), null, { timeout: 10000 }).then(() => true, () => false);
    check(needs, "the folders say what they need, from the mapfile (" + (await text("#facts-image")).slice(0, 60) + " | " + (await text("#facts-work")).slice(0, 60) + ")");
    // 4. Start.
    await page.click("#b-auto-start");
    await page.waitForFunction(() => /Running|Waiting|Stage \d+ of \d+/.test(document.getElementById("ast-start").textContent), null, { timeout: 10000 })
      .then(() => check(true, "Start runs"), () => check(false, "Start runs"));
    await page.waitForTimeout(1500);
    await shot("auto-2-running");
    // The window's title (which the taskbar shows) says how it is getting on.
    const running = await page.evaluate(() => [S.job && S.job.state, document.title]);
    if (running[0] === "running") check(running[1] !== "Automatic Recovery Probe" && running[1].endsWith(" · Automatic Recovery Probe"),
      "while Start runs, the window's title says how it gets on (" + running[1] + ")");
    await page.waitForFunction(() => /Done|Needs attention/.test(document.getElementById("ast-start").textContent), null, { timeout: 600000 });
    const st = await text("#ar-start");
    check(/Start is done/.test(st), "Start finishes: " + (st.match(/Start (is done|stopped)[^\n]*/) || [""])[0]);
    // What just happened, newest first, in "Now"; and the title says it is done.
    await page.waitForFunction(() => /Start is done/.test(document.getElementById("now-feed").innerText), null, { timeout: 10000 }).catch(() => {});
    const feed = await page.evaluate(() => [...document.querySelectorAll("#now-feed > ol.feed > li:not(.day)")].map((li) => li.innerText.replace(/\s+/g, " ")));
    check(feed.length >= 3 && /^\d\d:\d\d Start is done/.test(feed[0]) && feed.some((l) => /Stage 15 done/.test(l)),
      "Now says what just happened, newest first, each with its time (" + feed.slice(0, 2).join(" | ") + ")");
    check(await page.title() === "Start is done · Automatic Recovery Probe", "the window's title says Start is done (" + await page.title() + ")");
    const done = await page.locator("#ar-start ul.stages li.done").count();
    const skipped = await page.locator("#ar-start ul.stages li.skipped").count();
    check(done + skipped === 15, `every stage is done or skipped (${done} done, ${skipped} skipped)`);
    await shot("auto-3-done", true);
    // Afterwards: what WORK holds, from the list of copies, and the same in
    // the picture of where the files go.
    await page.evaluate(() => loadPlaces());
    const holds = await page.waitForFunction(() => /Holds/.test(document.getElementById("facts-work").textContent) &&
      document.querySelectorAll("#journey .node-facts .fact").length >= 4, null, { timeout: 10000 }).then(() => true, () => false);
    check(holds, "WORK says what it holds, and the picture shows the sizes (" + (await text("#facts-work")).slice(0, 70) + ")");
    // The live numbers, the rings and the time left, as the page shows them
    // (Start on the small test disk is over too quickly to catch them).
    const shown = await page.evaluate(() => {
      const L = { what: "copied", files: [1234, 5000], bytes: [5 * 1024 ** 3, 20 * 1024 ** 3],
        kinds: [{ name: "Photos", files: [1000, 4000], bytes: [4 * 1024 ** 3, 15 * 1024 ** 3] }, { name: "Videos", files: [234, 1000], bytes: [1024 ** 3, 5 * 1024 ** 3] }],
        now: "E:\\WORK\\DCIM\\2019\\IMG_0001.JPG" };
      const box = document.createElement("div");
      box.innerHTML = liveHtml(L, false);
      return { text: box.textContent.replace(/\s+/g, " "), rings: box.querySelectorAll("svg.ring").length,
        gb: [gb(300 * 1024 ** 2), gb(4.25 * 1024 ** 3), gb(931.5 * 1024 ** 3), gb(1863 * 1024 ** 3)],
        left: [leftText(30), leftText(600), leftText(7800), leftText(3 * 3600 + 58 * 60)], pct: [pct(0.375), pct(0.9999)] };
    });
    check(/5\.0 GB of 20 GB copied/.test(shown.text) && /1,234 of 5,000 files/.test(shown.text) && /Photos.*1,000 of 4,000/.test(shown.text) &&
      /…\\DCIM\\2019\\IMG_0001\.JPG/.test(shown.text) && shown.rings === 2,
      "the live numbers: GB and files done, by kind, each kind with its ring, and the file being worked on (" + shown.text.slice(0, 120) + ")");
    check(JSON.stringify(shown.gb) === JSON.stringify(["300 MB", "4.3 GB", "932 GB", "1,863 GB"]), "sizes are rounded to MB or GB (" + shown.gb.join(", ") + ")");
    check(JSON.stringify(shown.left) === JSON.stringify(["less than a minute left", "about 10 min left", "about 2 h 10 min left", "about 4 h left"]) &&
      JSON.stringify(shown.pct) === JSON.stringify(["37%", "99%"]), "the time left and the percentage read naturally (" + shown.left.join("; ") + ")");
    // When it will be done, as a time on the clock; what just happened, by day.
    const clock = await page.evaluate(() => {
      const yesterday = new Date(Date.now() - 864e5), two = (n) => String(n).padStart(2, "0");
      const day = `${yesterday.getFullYear()}-${two(yesterday.getMonth() + 1)}-${two(yesterday.getDate())} 21:40:00`;
      const box = document.createElement("div");
      box.innerHTML = feedHtml([{ when: day, tone: "good", text: "Done: the self-test" }]);
      return { soon: finishText(600), later: finishText(26 * 3600), feed: box.textContent.replace(/\s+/g, " ") };
    });
    // (Ten minutes from now is tomorrow when this runs just before midnight: then it is just after it.)
    const soon = /^done around (\d\d):\d\d( tomorrow)?$/.exec(clock.soon);
    check(soon && (!soon[2] || soon[1] === "00") &&
      /^done around \d\d:\d\d (tomorrow|on \w+day)$/.test(clock.later) &&
      /Yesterday\s*21:40\s*Done: the self-test/.test(clock.feed), "when it will be done reads as a time on the clock, and what happened by day (" + clock.soon + "; " + clock.later + ")");
    // The SSD's strip while the image is made: what is read, slowly, not at
    // all; the speed sagging; the temperature; and it fits a narrow window.
    const drive = await page.evaluate(() => {
      const size = 1000204886016;
      let cells = "";
      for (let i = 0; i < 400; i++) cells += i < 160 ? (i === 50 ? "2" : i === 90 ? "3" : "1") : "0";
      const speeds = [];
      for (let i = 0; i < 60; i++) speeds.push(i < 50 ? 400 : 120);
      const D = { size, pos: size * 0.4, cells, speeds, every: 2, slow: 1, bad_places: 1, bad_bytes: 4096, temp: 47, temp_max: 49, temp_limit: 70, temp_note: "", paused: false, pauses: 0, check: null };
      const realRunning = window.imageRunning;
      window.imageRunning = () => true;
      const box = document.createElement("div");
      box.style.width = "330px";
      document.body.appendChild(box);
      box.innerHTML = driveHtml(D, false);
      const r = { text: box.textContent.replace(/\s+/g, " "), fits: box.scrollWidth <= box.clientWidth + 1, strips: box.querySelectorAll("svg.drive-strip rect").length };
      box.innerHTML = driveHtml(Object.assign({}, D, { paused: true, temp: 71 }), false);
      r.paused = box.textContent.replace(/\s+/g, " ");
      box.remove();
      window.imageRunning = realRunning;
      return r;
    });
    check(/read slowly: 1 place/.test(drive.text) && /could not be read: 4 KB in 1 place/.test(drive.text) && /SSD at 47 °C/.test(drive.text) &&
      /much more slowly than before/.test(drive.text) && drive.strips >= 5 && drive.fits,
      "the SSD's strip says what was read, slowly or not at all, its speed sagging and its temperature, and fits a narrow window (" + drive.text.slice(0, 90) + ")");
    check(/Paused while the SSD cools down/.test(drive.paused) && !/much more slowly/.test(drive.paused), "and says when reading is paused while it cools down");
    // Windows notifications: on by default, and turned off and on again.
    check(await page.isChecked("#notify-on"), "Windows notifications are on to begin with");
    await page.uncheck("#notify-on");
    const off = await page.waitForFunction(() => S.notify === false && S.settings.notify === "off", null, { timeout: 10000 }).then(() => true, () => false);
    await page.check("#notify-on");
    const on = await page.waitForFunction(() => S.notify === true && S.settings.notify === "on", null, { timeout: 10000 }).then(() => true, () => false);
    check(off && on, "Windows notifications are turned off and on by their box");
    // The steps tab shows what Start did.
    await page.click("#tab-steps");
    check(/Done/.test(await text("#st-check")) && /Done/.test(await text("#st-copy")), "the steps tab shows the map check and the copying done");
    await page.click("#tab-auto");
    // 5. What is left.
    const left = await text("#ar-left");
    check(/repaired and put back|Nothing is left|hand-off/i.test(left), "What is left shows the repairs");
    // Photos before and after, if any photo was repaired: every picture shows.
    const pics = await page.evaluate(async () => {
      // (Shown lazily, as they come into view: here all at once.)
      const imgs = [...document.querySelectorAll("#ar-left .pair img")];
      imgs.forEach((i) => { i.loading = "eager"; });
      const loaded = Promise.all(imgs.map((i) => (i.complete ? null : new Promise((r) => { i.addEventListener("load", r); i.addEventListener("error", r); }))));
      await Promise.race([loaded, new Promise((r) => setTimeout(r, 15000))]);
      await new Promise((r) => setTimeout(r, 200));
      // (One the browser cannot draw is replaced by words saying so.)
      return [...document.querySelectorAll("#ar-left .pair img")].map((i) => i.naturalWidth);
    });
    if (pics.length) check(pics.every((w) => w > 0), "the photos before and after their repair show, or say why not (" + pics.length + ")");
    // 6. Finalise.
    await page.click("#b-finalise");
    await page.waitForFunction(() => /Done|Needs attention/.test(document.getElementById("ast-final").textContent), null, { timeout: 300000 });
    check(/Finalised/.test(await text("#ar-final")), "Finalise finishes");
    await shot("auto-4-finalised", true);
    // Every icon on the page comes with words saying what it is for.
    check((await page.locator("#rail .rail-item").count()) === 7, "the list on the left names the seven parts");
    // 7. Put Windows back: its own part, after Finalise (here nothing was changed).
    await page.evaluate(() => loadBack());
    const back = await page.waitForFunction(() => /Nothing to put back|Done/.test(document.getElementById("ast-back").textContent) &&
      /Windows is as it was/.test(document.getElementById("ar-back").textContent), null, { timeout: 10000 }).then(() => true, () => false);
    check(back, "7. Put Windows back as it was says Windows is as it was (" + (await text("#ar-back")).slice(0, 80) + ")");
    check(!/puts back the Windows settings/.test(await text("#a-final")), "Finalise no longer says it puts Windows' settings back");
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
