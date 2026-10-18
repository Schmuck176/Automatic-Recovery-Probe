// Drives the step-by-step page in a real browser (Chromium, through
// Playwright) against `recovery-probe ui` started with a test disk image,
// the way a person would: every step, start to finish. Takes screenshots.
//
//   node tools/ui_check.js <recovery-probe> <work folder> <test disk image> <mapfile>
const { spawn } = require("child_process");
const fs = require("fs");
const path = require("path");
let chromium;
for (const p of ["playwright", "/home/claude/.npm-global/lib/node_modules/@playwright/mcp/node_modules/playwright"]) {
  try { ({ chromium } = require(p)); break; } catch (e) { /* next */ }
}
const [exe, work, disk, mapfile] = process.argv.slice(2);
const home = path.join(work, "home");
fs.rmSync(work, { recursive: true, force: true });
fs.mkdirSync(home, { recursive: true });
fs.copyFileSync(mapfile, path.join(home, "second attempt.log"));
const copyTo = path.join(work, "copies");
const port = 47000 + Math.floor(Math.random() * 900);
const key = "uitestkey0123456789abcdef";
const results = [];
const pageErrors = [];
const check = (ok, what) => { results.push([!!ok, what]); console.log((ok ? "ok   " : "FAIL ") + what); };

(async () => {
  const server = spawn(exe, ["ui", "--port", String(port), "--token", key, "--no-browser", "--test-disk", disk, "--home", home],
    { stdio: ["ignore", "pipe", "pipe"] });
  let out = "";
  const log = fs.createWriteStream(path.join(work, "server-output.txt"));
  server.stdout.on("data", (d) => { out += d; log.write(d); });
  server.stderr.on("data", (d) => { log.write(d); });
  for (let i = 0; i < 50 && !out.includes("http://"); i++) await new Promise((r) => setTimeout(r, 100));
  const url = `http://127.0.0.1:${port}/${key}/`;
  check(out.includes(url), "the program prints the page's address");
  const browser = await chromium.launch();
  const page = await browser.newPage({ viewport: { width: 1180, height: 900 } });
  const errors = [];
  page.on("pageerror", (e) => { errors.push(String(e)); pageErrors.push(String(e)); });
  page.on("console", (m) => { if (m.type() === "error") { errors.push(m.text()); pageErrors.push(m.text()); } });
  const shot = (name) => page.screenshot({ path: path.join(work, name + ".png"), fullPage: true });
  const waitJob = async (step) => {
    await page.waitForFunction((s) => {
      const st = document.getElementById("st-" + s);
      return st && /Done|Needs attention|Stopped/.test(st.textContent);
    }, step, { timeout: 120000 });
  };

  // Someone without the key, or from another site, gets nothing.
  const bad = await fetch(`http://127.0.0.1:${port}/wrongkey/api/state`).then((r) => r.status);
  check(bad === 404, "a wrong key gets nothing (" + bad + ")");
  const post = await fetch(url + "api/do", { method: "POST", body: "action=selftest" }).then((r) => r.status);
  check(post === 403, "an action without the key header is refused (" + post + ")");

  // How quickly the program answers (reading every disk is the slow one).
  for (const api of ["api/state", "api/find-mapfiles", "api/disks"]) {
    const t0 = Date.now();
    const r = await fetch(url + api);
    await r.text();
    const ms = Date.now() - t0;
    console.log(`     ${api} answered in ${ms} ms`);
    if (api === "api/disks") check(ms < 5000, "the program lists the disks quickly (" + ms + " ms)");
  }
  await page.goto(url);
  await page.waitForSelector("#map-picture svg", { timeout: 10000 });
  check((await page.textContent("#map-lead")).includes("never read"), "the map shows the unread places");
  check(await page.isChecked("input[name=mapfile]"), "the mapfile in the program's folder is chosen by itself");
  await shot("1-start");

  // Step 1.
  await page.click("#b-test");
  await waitJob("test");
  check((await page.textContent("#r-test")).includes("Every check passed"), "step 1: the self-test passes");

  // Step 3: the test disk.
  const diskRadio = page.locator("input[name=disk]").last();
  await diskRadio.check();
  await page.waitForFunction(() => /Test disk/.test(document.getElementById("st-connect").textContent), null, { timeout: 10000 });
  check(true, "step 3: the test disk can be chosen");
  check((await page.textContent("#connect")).includes("the copy ddrescue made"), "step 3: the test disk is recognised as the copy");
  // Another disk (on GitHub's Windows machine, its second drive): nothing
  // to protect there, and no button that would.
  const other = await page.evaluate(() => {
    const radios = [...document.querySelectorAll("input[name=disk]")];
    const i = radios.findIndex((r) => !r.disabled && !r.closest("label").textContent.includes("test disk image"));
    return i;
  });
  if (other >= 0) {
    await page.locator("input[name=disk]").nth(other).check();
    const told = await page.waitForFunction(() => document.getElementById("connect").textContent.includes("needs no protecting"),
      null, { timeout: 10000 }).then(() => true, () => false);
    check(told && (await page.locator("#b-protect").count()) === 0, "step 3: another disk gets no Protect button");
    await diskRadio.check();
    await page.waitForFunction(() => /Test disk/.test(document.getElementById("st-connect").textContent), null, { timeout: 10000 });
  }

  // Step 4.
  await page.click("#b-check");
  await waitJob("check");
  const r4 = await page.textContent("#r-check");
  check(r4.includes("files lost data") && r4.includes("IMG_0003.JPG"), "step 4: the damaged photo is named");
  check(r4.includes("letter 11.docx") || r4.includes("letter 12.docx"), "step 4: files Windows cannot list are shown");
  check((await page.locator("#map-legend").textContent()).includes("folder records"), "the map now says what each place held");
  await shot("2-map-check");

  // Step 5.
  await page.fill("#copy-to", copyTo);
  await page.click("#b-copy");
  await waitJob("copy");
  const r5 = await page.textContent("#r-copy");
  const want = fs.readFileSync(path.join(__dirname, "..", "src", "selftest", "expected.tsv"), "utf8")
    .split("\n").filter((l) => l.startsWith("FOUND\t") && l.split("\t")[2] === "0").length;
  check(r5.includes(`Copied ${want} files`), `step 5: all ${want} files are copied (` + r5.slice(0, 60) + ")");
  check(fs.existsSync(path.join(copyTo, "Photos", "Summer 2019_")), "step 5: a name Windows cannot use is made safe");
  check(fs.existsSync(path.join(copyTo, "Documents", "letter 15.docx")), "step 5: a file Windows cannot list is copied");

  // Step 6.
  check((await page.inputValue("#scan-folder")) === copyTo, "step 6: the folder to check is the one step 5 copied into");
  await page.click("#b-scan");
  await waitJob("scan");
  const r6 = await page.textContent("#r-scan");
  check(r6.includes("NEVER_READ"), "step 6: the damaged photo is marked NEVER_READ");
  await page.click("#r-scan details summary");
  await shot("3-checked");

  // A job that cannot start says why.
  await page.fill("#copy-to", "");
  // (Clicked through the page itself: on Windows the page sometimes moves
  // under the pointer while results redraw.)
  await page.evaluate(() => document.getElementById("b-copy").click());
  const refused = await page.waitForFunction(
    () => document.getElementById("r-copy").textContent.includes("Choose where the copies go first"), null, { timeout: 5000 })
    .then(() => true, () => false);
  check(refused, "an empty folder is refused with a reason");
  if (!refused) await shot("fail-empty-folder");

  // Closing the program: the page says so. (On Windows a refused connection
  // takes a couple of seconds to fail, so allow some time.)
  server.kill();
  const told = await page.waitForFunction(
    () => document.getElementById("restore-banner").textContent.includes("not running any more"), null, { timeout: 15000 })
    .then(() => true, () => false);
  check(told, "while the program is closed, the page says so");
  if (!told) await shot("fail-closed");
  errors.length = 0; // (the page's own requests failed while it was closed)

  // The state survives a restart of the program.
  const server2 = spawn(exe, ["ui", "--port", String(port), "--token", key, "--no-browser", "--test-disk", disk, "--home", home],
    { stdio: "ignore" });
  await new Promise((r) => setTimeout(r, 1500));
  await page.goto(url);
  await page.waitForFunction(() => /Done/.test(document.getElementById("st-check").textContent), null, { timeout: 10000 });
  check(true, "after closing and opening the program, step 4's results are still there");

  // Phone-sized screen: nothing wider than the screen.
  await page.setViewportSize({ width: 390, height: 844 });
  await page.reload();
  await page.waitForSelector("#map-picture svg");
  const wide = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  check(wide <= 1, "on a phone-sized window nothing sticks out sideways (" + wide + " px)");
  await shot("4-narrow");
  await page.emulateMedia({ colorScheme: "dark" });
  await page.setViewportSize({ width: 1180, height: 900 });
  await shot("5-dark");

  check(errors.length === 0, "no errors in the page" + (errors.length ? ": " + errors.slice(0, 3).join(" | ") : ""));
  const others = pageErrors.filter((x) => !/ERR_CONNECTION_REFUSED|Failed to fetch|Failed to load resource/.test(x));
  check(others.length === 0, "no errors in the page at any point" + (others.length ? ": " + others.slice(0, 3).join(" | ") : ""));
  await browser.close();
  server2.kill();
  const failed = results.filter((r) => !r[0]).length;
  console.log(`Page checks: ${results.length - failed} of ${results.length} passed`);
  process.exit(failed ? 1 : 0);
})().catch((e) => {
  console.log("FAIL " + e);
  for (const x of pageErrors) console.log("     page said: " + x);
  process.exit(1);
});
