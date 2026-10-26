// Drives the step-by-step page in a real browser (Chromium, through
// Playwright) against `recovery-probe ui` started with a test disk image,
// the way a person would: every step, start to finish. Takes screenshots.
//
//   node tools/ui_check.js <recovery-probe> <work folder> <test disk image> <mapfile>
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
// The person's own copy of the checklist (in Obsidian), with its first box ticked there.
const obsidian = path.join(home, "Obsidian MD files", "automatic-recovery-checklist.md");
fs.mkdirSync(path.dirname(obsidian), { recursive: true });
fs.writeFileSync(obsidian, fs.readFileSync(path.join(__dirname, "..", "docs", "automatic-checklist.md"), "utf8").replace("- [ ] ", "- [x] "));
// Recovery Probe's own checklist beside it is never touched.
const oldList = path.join(home, "Obsidian MD files", "recovery-checklist.md");
fs.writeFileSync(oldList, "- [x] Recovery Probe's own checklist, with the person's ticks\n");
const copyTo = path.join(work, "copies");
const imageTo = path.join(work, "image");
// A drive exactly as big as the old one that Windows set up for itself, as
// SPARE is if it is a 1 TB drive: the test disk's partition table (made for
// the old drive's size, as every drive that size has) and nothing else.
const spare = path.join(work, "spare as big as the old drive.img");
{
  const d = fs.readFileSync(disk);
  const s = Buffer.alloc(d.length);
  d.copy(s, 0, 0, 34 * 512);
  d.copy(s, d.length - 33 * 512, d.length - 33 * 512);
  fs.writeFileSync(spare, s);
}
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
const key = "uitestkey0123456789abcdef";
// Icons shown with no words beside them (in their own element or the one
// around it), which the page must never have. (Hidden ones don't count.)
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
const results = [];
const pageErrors = [];
const check = (ok, what) => { results.push([!!ok, what]); console.log((ok ? "ok   " : "FAIL ") + what); };

(async () => {
  port = await freePort();
  const server = spawn(exe, ["ui", "--port", String(port), "--token", key, "--no-browser", "--test-disk", spare, "--test-disk", disk, "--home", home],
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
  // (The checklist is a long page: a picture of the window, not of the whole page,
  // which Windows' browser can take too long to make.)
  const shotView = (name) => page.screenshot({ path: path.join(work, name + ".png") });
  const waitJob = async (step) => {
    await page.waitForFunction((s) => {
      const st = document.getElementById("st-" + s);
      return st && /Done|Needs attention|Stopped/.test(st.textContent);
    }, step, { timeout: 120000 });
  };
  const waitText = (id, re, ms) => page.waitForFunction(([i, r]) => new RegExp(r).test(document.getElementById(i).textContent),
    [id, re], { timeout: ms || 60000 }).then(() => true, () => false);

  // Someone without the key, or from another site, gets nothing. (Every
  // answer is read to its end: Node's fetch can trip over one left unread
  // when the program closes the connection - "assert(!this.paused)".)
  const statusOf = (r) => r.text().then(() => r.status);
  const bad = await fetch(`http://127.0.0.1:${port}/wrongkey/api/state`).then(statusOf);
  check(bad === 404, "a wrong key gets nothing (" + bad + ")");
  const post = await fetch(url + "api/do", { method: "POST", body: "action=selftest" }).then(statusOf);
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
  // The page opens on Automatic; the checklist is built in, in its own tab.
  await page.waitForSelector("#view-auto:not([hidden]) #b-ready", { timeout: 10000 });
  check(true, "the page opens on the Automatic tab");
  await page.click("#tab-checklist");
  await page.waitForSelector("#view-checklist:not([hidden]) input.tick", { timeout: 10000 });
  await page.waitForFunction(() => /\(\d+ of \d+\)/.test(document.getElementById("tab-count").textContent), null, { timeout: 10000 });
  const tab0 = await page.textContent("#tab-count");
  check(await page.isChecked('input.tick[data-i="0"]') && /^\(1 of \d+\)$/.test(tab0),
    "the checklist is built in, with the box ticked in the copy in Obsidian taken in " + tab0);
  check((await page.textContent("#where")).includes("Next on the checklist"), "the page says what is next on the checklist");
  await page.click('input.tick[data-i="1"]');
  await page.waitForFunction(() => /^\(2 of /.test(document.getElementById("tab-count").textContent), null, { timeout: 10000 });
  check((fs.readFileSync(obsidian, "utf8").match(/- \[x\] /g) || []).length === 2 && fs.existsSync(path.join(home, "results", "automatic-checklist-ticks.txt")) &&
    fs.readFileSync(oldList, "utf8") === "- [x] Recovery Probe's own checklist, with the person's ticks\n",
    "a box ticked on the page is kept, and the copy in Obsidian follows");
  check(fs.existsSync(path.join(home, "Obsidian MD files", "automatic-download-manifest.md")) && !fs.existsSync(path.join(home, "Obsidian MD files", "download-manifest.md")), "the download manifest is put beside it (and not Recovery Probe's)");
  await shotView("0-checklist");
  await page.click("#tab-steps");
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
  // A drive exactly as big as the old one, set up for itself (SPARE), is not
  // taken for the copy.
  const listed = await fetch(url + "api/disks").then((r) => r.json());
  const asSpare = listed.find((d) => d.spec === spare), asCopy = listed.find((d) => d.spec === disk);
  check(asSpare && asSpare.detail && asSpare.detail.table_disk_size === asSpare.size && asSpare.detail.matches_mapfile === false &&
    asCopy && asCopy.detail && asCopy.detail.matches_mapfile === true,
    "step 3: a drive exactly as big as the old one, set up for itself (like SPARE), is not taken for the copy");
  const lineOf = (v) => page.evaluate((s) => {
    const r = [...document.querySelectorAll("input[name=disk]")].find((x) => x.value === s);
    return r ? r.closest("label").textContent.replace(/\s+/g, " ").trim() : "";
  }, v);
  const spareLine = await lineOf(spare);
  check(spareLine && !spareLine.includes("the copy ddrescue made") && !spareLine.includes("partition table made for") &&
    (await lineOf(disk)).includes("the copy ddrescue made"), "step 3: only the copy is marked as the copy (" + spareLine.slice(0, 70) + ")");
  // Clicked by hand, it stays chosen while the copy is there; a copy plugged
  // in after another drive was clicked is chosen by itself.
  await page.evaluate((s) => [...document.querySelectorAll("input[name=disk]")].find((x) => x.value === s).click(), spare);
  await page.waitForFunction((s) => S && S.settings.disk === s, spare, { timeout: 10000 });
  await page.evaluate(() => loadDisks());
  check((await page.evaluate(() => S.settings.disk)) === spare, "step 3: a drive clicked by hand stays chosen while the copy is there");
  const choice = await page.evaluate(([s, c]) => {
    const sp = disks.find((d) => d.spec === s), cp = disks.find((d) => d.spec === c);
    return [autoChoice([sp, cp], s, s, []), autoChoice([sp, cp], s, s, [c]), autoChoice([sp, cp], "", "", []), autoChoice([sp], s, s, []),
      autoChoice([sp, cp, Object.assign({}, cp, { spec: "another copy" })], "", "", [])];
  }, [spare, disk]);
  check(JSON.stringify(choice) === JSON.stringify([disk, null, disk, null, null]),
    "step 3: the copy is chosen when it arrives after another drive was clicked, and not when it was there already (" + JSON.stringify(choice) + ")");
  await diskRadio.check();
  await page.waitForFunction((c) => S && S.settings.disk === c, disk, { timeout: 10000 });
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

  // Step 4: the image.
  await page.fill("#image-to", imageTo);
  await page.click("#b-image");
  await waitJob("image");
  const r4i = await page.textContent("#r-image");
  check(r4i.includes("reads back exactly as it was written"), "step 4: the image is made and checked (" + r4i.slice(0, 80) + ")");
  check(fs.existsSync(path.join(imageTo, "recovered-ssd.img")) && fs.existsSync(path.join(imageTo, "recovered-ssd.img.sha256")),
    "step 4: the image and its checksum are there");
  check(fs.readFileSync(path.join(imageTo, "recovered-ssd.img")).equals(fs.readFileSync(disk).subarray(0, fs.statSync(path.join(imageTo, "recovered-ssd.img")).size)),
    "step 4: the image holds exactly the disk's bytes");
  await page.click("#b-imagecheck");
  check(await waitText("r-imagecheck", "exactly as it was made"), "step 4: the image can be checked again later, without the SSD");
  await page.waitForFunction(() => /Reads from the image/.test(document.getElementById("check-source").textContent), null, { timeout: 10000 })
    .then(() => check(true, "step 5 reads the image from now on"), () => check(false, "step 5 reads the image from now on"));

  // Step 5.
  await page.click("#b-check");
  await waitJob("check");
  const r4 = await page.textContent("#r-check");
  check(r4.includes("files lost data") && r4.includes("IMG_0003.JPG"), "step 5: the damaged photo is named");
  check(r4.includes("letter 11.docx") || r4.includes("letter 12.docx"), "step 5: files Windows cannot list are shown");
  check((await page.locator("#map-legend").textContent()).includes("folder records"), "the map now says what each place held");
  check(/files on the SSD, [\d.,]+ (KB|MB|GB) in all/.test(r4), "step 5: how much the files take in all is shown");
  // The picture of where the files go says what the repairs will need, from the map check.
  const rep = await page.evaluate(() => { const d = lastOf("mapcheck").data; return [d.files_losing_data, d.losing_bytes, document.getElementById("journey").textContent]; });
  check(rep[0] > 0 && rep[1] > 0 && new RegExp(rep[0] + " damaged files? so far").test(rep[2].replace(/,/g, "")),
    "the repairs say how many damaged files there are so far, and their size, from the map check (" + rep[0] + " files, " + rep[1] + " bytes)");
  await page.evaluate(() => loadChecklist());
  const nums = [await page.textContent('#cl-body td[data-num="files"]'), await page.textContent('#cl-body td[data-num="bytes"]')];
  check(nums[0] === "96" && /^[\d.,]+ KB$/.test(nums[1]), "the checklist's Your numbers are filled in from step 5 (" + nums.join(", ") + ")");
  // A part the person folds out stays open while the page refreshes itself.
  await page.locator("#r-check details > summary").first().click();
  await page.waitForTimeout(4500);
  check(await page.locator("#r-check details").first().evaluate((d) => d.open), "step 5: the full report stays open while the page refreshes");
  await page.locator("#r-check details > summary").first().click();
  await shot("2-map-check");

  // Step 6.
  await page.fill("#copy-to", copyTo);
  await page.click("#b-copy");
  await waitJob("copy");
  const r5 = await page.textContent("#r-copy");
  const want = fs.readFileSync(path.join(__dirname, "..", "src", "selftest", "expected.tsv"), "utf8")
    .split("\n").filter((l) => l.startsWith("FOUND\t") && l.split("\t")[2] === "0").length;
  check(r5.includes(`Copied ${want} files`), `step 6: all ${want} files are copied (` + r5.slice(0, 60) + ")");
  check(fs.existsSync(path.join(copyTo, "Photos", "Summer 2019_")), "step 6: a name Windows cannot use is made safe");
  check(fs.existsSync(path.join(copyTo, "Documents", "letter 15.docx")), "step 6: a file Windows cannot list is copied");
  check(r5.includes("Safety check") && r5.includes("Every file step 5 found on the SSD is in the list of copies"),
    "step 6: the safety check finds every file step 5 found in the list of copies (" +
    (r5.slice(r5.indexOf("Safety check"), r5.indexOf("Safety check") + 140)) + ")");

  // Step 7.
  check((await page.inputValue("#scan-folder")) === copyTo, "step 7: the folder to check is the one step 6 copied into");
  await page.click("#b-scan");
  await waitJob("scan");
  const r6 = await page.textContent("#r-scan");
  check(r6.includes("NEVER_READ"), "step 7: the damaged photo is marked NEVER_READ");
  await page.click("#r-scan details summary");
  await shot("3-checked");
  // The checklist's first checkpoint: once the boxes before it are ticked, it is due.
  const cp = await page.evaluate(async () => {
    const c = CL.items.findIndex((it) => it.checkpoint);
    for (let i = 0; i < c; i++) if (CL.items[i].phase === CL.items[c].phase) await act({ action: "tick", i: String(i), on: "1" });
    await loadChecklist();
    return c;
  });
  const cpState = () => page.evaluate((c) => CLS.status.find((s) => s.i === c).state, cp);
  check((await cpState()) === "due" && (await page.textContent("#where")).includes("Checkpoint due"),
    "checklist: the checkpoint is due once the boxes before it are ticked");
  const byHand = await page.evaluate(async (c) => { try { await act({ action: "tick", i: String(c), on: "1" }); return "ticked"; } catch (e) { return e.message; } }, cp);
  check(byHand.includes("ticks itself"), "checklist: a checkpoint can't be ticked by hand while it is due (" + byHand.slice(0, 60) + ")");
  // The copies against the checksums the copying wrote.
  await page.click("#b-verify");
  const verified = await page.waitForFunction(() => /exactly what was copied/.test(document.getElementById("r-verify").textContent),
    null, { timeout: 60000 }).then(() => true, () => false);
  check(verified, "step 7: every copy is exactly what was copied (" + (await page.textContent("#r-verify")).slice(0, 80) + ")");
  const rv = await page.textContent("#r-verify");
  check(rv.includes("Nothing is missing, and nothing has changed") && (await page.locator("#r-verify .safety:not(.history) td.mark.good").count()) === 3,
    "step 7: the safety check has the same files at all three stages (" + rv.slice(rv.indexOf("Safety check"), rv.indexOf("Safety check") + 160) + ")");
  await shot("3a-safety-check");
  await page.evaluate(() => loadChecklist());
  check((await cpState()) === "passed" && (await page.evaluate((c) => CLS.ticked[c], cp)) === 1,
    "checklist: the checkpoint ticks itself when the check passes");
  // A copy deleted by mistake: the safety check says at which stage, and names it.
  const victim = path.join(copyTo, "Big folder", "file number 03 with a longer name.bin");
  const kept = fs.readFileSync(victim);
  fs.unlinkSync(victim);
  await page.click("#b-verify");
  const caught = await waitText("r-verify", "1 missing");
  const rd = await page.textContent("#r-verify");
  check(caught && rd.includes("file number 03 with a longer name.bin") && rd.includes("after step 6") &&
    (await page.locator("#r-verify .safety:not(.history) td.mark.bad").count()) === 1,
    "step 7: a copy deleted by mistake is caught at the last stage, and named (" + rd.slice(0, 90) + ")");
  await page.evaluate(() => loadChecklist());
  check((await cpState()) === "failed" && (await page.textContent("#where")).includes("Stop."),
    "checklist: after a check that found something, the checkpoint says stop");
  const gate = await page.evaluate(async (f) => { try { await act({ action: "put-back", folder: f }); return "started"; } catch (e) { return e.message; } }, copyTo);
  check(gate.includes("Put back waits for a check that passes"), "step 8: put back is refused while the last check found something (" + gate.slice(0, 70) + ")");
  fs.writeFileSync(victim, kept);
  await page.click("#b-verify");
  check(await waitText("r-verify", "Nothing is missing, and nothing has changed"), "step 7: with it back, nothing is missing again");
  const rh = await page.textContent("#r-verify");
  check(rh.includes("Every safety check so far") && (await page.locator("#r-verify .history td.mark.good").count()) === 4 &&
    (await page.locator("#r-verify .history td.mark.bad").count()) === 1,
    "step 7: every check so far is listed - the image, the copying, a check, the one that found the deleted copy (✗), and the check after");
  const historyCsv = path.join(home, "results", "safety-checks.csv");
  check(fs.existsSync(historyCsv) && fs.readFileSync(historyCsv, "utf8").split("\n").filter((l) => l.trim()).length === 6,
    "step 7: every check is in results\\safety-checks.csv, with all its numbers");
  await page.evaluate(() => loadChecklist());
  check((await cpState()) === "passed", "checklist: with the file back and a check that passes, the checkpoint is ticked again");

  // Step 8: isolate, repair, put back, evaluate, undo.
  const repairDir = path.join(path.dirname(copyTo), "REPAIR");
  await page.click("#b-isolate");
  const isolated = await waitText("r-isolate", "Copied \\d+ damaged files out");
  check(isolated, "step 8: the damaged files are copied out (" + (await page.textContent("#r-isolate")).slice(0, 90) + ")");
  const photoGroup = path.join(repairDir, "to-repair", "1 Photos (photo repair tool)");
  const tagged = fs.existsSync(photoGroup) ? fs.readdirSync(photoGroup).find((n) => /^IMG_0003 \[R\d+\]\.JPG$/.test(n)) : undefined;
  check(tagged && fs.existsSync(path.join(repairDir, "repair-list.csv")),
    "step 8: the damaged photo is in REPAIR with its number (" + tagged + "), and repair-list.csv says where it belongs");
  const original = fs.readFileSync(path.join(copyTo, "Photos", "IMG_0003.JPG"));
  if (tagged) {
    // "Repair" it: a version with the never-read bytes filled in, saved the way repair tools name things.
    const fixed = Buffer.from(fs.readFileSync(path.join(photoGroup, tagged)));
    for (let i = 0; i < fixed.length; i++) if (fixed[i] === 0) fixed[i] = 0x55;
    fs.writeFileSync(path.join(repairDir, "repaired", tagged.replace(/\.JPG$/, "_repaired.JPG")), fixed);
    await page.click("#b-putback");
    const back = await waitText("r-putback", "Put back 1 repaired files");
    check(back, "step 8: the repaired photo is put back (" + (await page.textContent("#r-putback")).slice(0, 90) + ")");
    check(fs.readFileSync(path.join(copyTo, "Photos", "IMG_0003.JPG")).equals(fixed),
      "step 8: WORK now holds the repaired version, at the same place");
    check(fs.readFileSync(path.join(repairDir, "originals", "Photos", "IMG_0003.JPG")).equals(original),
      "step 8: the damaged original is kept in REPAIR\\originals");
    await page.click("#b-verify");
    const okAfter = await waitText("r-verify", "exactly the repaired version put back");
    check(okAfter, "step 7 again: the copies check knows about the repaired version (" + (await page.textContent("#r-verify")).slice(0, 100) + ")");
    check((await page.textContent("#r-verify")).includes("exactly as copied or as repaired and put back"),
      "step 7 again: the safety check counts the repaired photo as here");
    await page.click("#b-evaluate");
    const evaluated = await waitText("r-evaluate", "damaged files:");
    check(evaluated, "step 8: the evaluation says what is put back and what is still damaged (" + (await page.textContent("#r-evaluate")).slice(0, 100) + ")");
    await page.fill("#undo-id", tagged.match(/\[(R\d+)\]/)[1]);
    await page.click("#b-undo");
    const undone = await waitText("r-putback", "is back in WORK");
    check(undone && fs.readFileSync(path.join(copyTo, "Photos", "IMG_0003.JPG")).equals(original),
      "step 8: undo puts the original back in WORK");
  }
  await shot("3b-repair");
  const mute = await iconsWithoutWords(page);
  check(mute.length === 0, "the steps tab: every icon has words beside it" + (mute.length ? ": " + mute.slice(0, 5).join(" | ") : ""));
  // "Step 7" in the checklist goes to step 7 (not the 7th numbered part on the page).
  await page.click("#tab-checklist");
  await page.evaluate(() => goStep(7));
  await page.waitForTimeout(400);
  const top7 = await page.evaluate(() => Math.round(document.getElementById("s-scan").getBoundingClientRect().top));
  check(Math.abs(top7) < 60, "the checklist's “step 7” goes to step 7 (" + top7 + " px from the top)");

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
  await page.evaluate(() => loadChecklist());
  const ticksBefore = await page.evaluate(() => CLS.done);
  server.kill();
  const told = await page.waitForFunction(
    () => document.getElementById("restore-banner").textContent.includes("not running any more"), null, { timeout: 15000 })
    .then(() => true, () => false);
  // ...and keeps saying so while it goes on looking for disks.
  await page.waitForTimeout(4000);
  const stays = (await page.textContent("#restore-banner")).includes("not running any more");
  check(told && stays, "while the program is closed, the page says so, and keeps saying it (" + told + ", " + stays + ")");
  if (!told) await shot("fail-closed");

  // The state survives a restart of the program.
  const server2 = spawn(exe, ["ui", "--port", String(port), "--token", key, "--no-browser", "--test-disk", spare, "--test-disk", disk, "--home", home],
    { stdio: "ignore" });
  // Until it answers, the page's own requests fail, as they should.
  for (let i = 0; i < 100; i++) {
    const up = await fetch(url).then((r) => r.text().then(() => r.ok), () => false);
    if (up) break;
    await new Promise((r) => setTimeout(r, 200));
  }
  errors.length = 0; // (the page's own requests failed while the program was closed)
  await page.goto(url);
  await page.waitForFunction(() => /Done/.test(document.getElementById("st-check").textContent), null, { timeout: 10000 });
  check(true, "after closing and opening the program, step 5's results are still there");
  await page.waitForFunction(() => /\(\d+ of \d+\)/.test(document.getElementById("tab-count").textContent), null, { timeout: 10000 });
  const ticksAfter = await page.evaluate(() => CLS.done);
  check(ticksAfter === ticksBefore && ticksAfter >= 5, "the checklist's ticks are still there too (" + ticksAfter + " of " + ticksBefore + ")");

  // Phone-sized screen: nothing wider than the screen.
  await page.setViewportSize({ width: 390, height: 844 });
  await page.reload();
  await page.waitForSelector("#map-picture svg");
  const wide = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  check(wide <= 1, "on a phone-sized window nothing sticks out sideways (" + wide + " px)");
  await shot("4-narrow");
  await page.click("#tab-checklist");
  await page.waitForSelector("#view-checklist:not([hidden]) input.tick");
  const wideList = await page.evaluate(() => document.documentElement.scrollWidth - window.innerWidth);
  check(wideList <= 1, "the checklist on a phone-sized window: nothing sticks out sideways (" + wideList + " px)");
  await shotView("4b-narrow-checklist");
  await page.click("#tab-steps");
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
