// Smoke test for the Node addon: feed bytes, read back the parsed block list.
// Run after building: see the build step in the repo README / CI.
const assert = require("node:assert");
const path = require("node:path");

const addonPath = process.env.SPACETERM_ADDON || path.join(__dirname, "spaceterm.node");
const { Terminal } = require(addonPath);

// Plain text flows straight through to the scrollback.
const term = new Terminal();
term.feed(Buffer.from("hello world"));
assert.strictEqual(term.plainText(), "hello world");

// An iTerm2 inline PNG is normalized into an image content block.
const png = Buffer.from([0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a]).toString("base64");
term.feed(Buffer.from(`\x1b]1337;File=inline=1:${png}\x1b\\`, "latin1"));

const blocks = JSON.parse(term.blocksJson());
const hasImage = blocks.some((block) =>
  block.output.some((seg) => seg.kind === "content" && seg.data.bundle.mime["image/png"]),
);
assert.ok(hasImage, "expected an image/png content block: " + term.blocksJson());

console.log("bindings smoke test passed");
