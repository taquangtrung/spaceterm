import { WebglAddon } from "@xterm/addon-webgl";
import { Terminal } from "@xterm/xterm";

// ========================================================================
// Globals injected by the Rust host
// ========================================================================

declare global {
  interface Window {
    __CORPUS_B64__: string;
    __CHUNK_BYTES__: number;
    __RUNS__: number;
    __WARMUP__: number;
    ipc?: { postMessage(message: string): void };
  }
}

// ========================================================================
// Helpers
// ========================================================================

function decodeCorpus(b64: string): Uint8Array {
  const binary = atob(b64);
  const bytes = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) {
    bytes[i] = binary.charCodeAt(i);
  }
  return bytes;
}

function percentile(sorted: number[], p: number): number {
  if (sorted.length === 0) {
    return 0;
  }
  const idx = Math.min(sorted.length - 1, Math.round((sorted.length - 1) * p));
  return sorted[idx];
}

function summarize(values: number[]): { median: number; min: number; max: number } {
  if (values.length === 0) {
    return { median: 0, min: 0, max: 0 };
  }
  const sorted = [...values].sort((a, b) => a - b);
  return {
    median: sorted[Math.floor(sorted.length / 2)],
    min: sorted[0],
    max: sorted[sorted.length - 1],
  };
}

// ========================================================================
// Single pass
// ========================================================================

interface RunResult {
  frameP99Ms: number;
  throughputMbS: number;
}

/// Write the whole corpus once (one chunk per call) and resolve with the timing.
function runOnce(term: Terminal, corpus: Uint8Array, chunkBytes: number): Promise<RunResult> {
  return new Promise((resolve) => {
    const total = corpus.length;
    const frameTimes: number[] = [];
    let collecting = true;
    let lastFrame = performance.now();
    function collectFrames(): void {
      const now = performance.now();
      frameTimes.push(now - lastFrame);
      lastFrame = now;
      if (collecting) {
        requestAnimationFrame(collectFrames);
      }
    }

    let offset = 0;
    const start = performance.now();
    lastFrame = start;
    requestAnimationFrame(collectFrames);

    function writeNext(): void {
      if (offset >= total) {
        // One more frame so the final write is painted before we stop the clock.
        requestAnimationFrame(() => {
          const elapsedMs = performance.now() - start;
          collecting = false;
          frameTimes.sort((a, b) => a - b);
          resolve({
            frameP99Ms: percentile(frameTimes, 0.99),
            throughputMbS: total / (1024 * 1024) / (elapsedMs / 1000),
          });
        });
        return;
      }
      const end = Math.min(offset + chunkBytes, total);
      const chunk = corpus.subarray(offset, end);
      offset = end;
      term.write(chunk, writeNext);
    }
    writeNext();
  });
}

// ========================================================================
// Benchmark
// ========================================================================

async function run(): Promise<void> {
  const host = document.getElementById("term");
  if (!host) {
    return;
  }

  const term = new Terminal({ convertEol: false, fontFamily: "monospace", scrollback: 0 });
  term.open(host);
  let backend = "canvas";
  try {
    term.loadAddon(new WebglAddon());
    backend = "webgl";
  } catch {
    backend = "canvas";
  }

  const corpus = decodeCorpus(window.__CORPUS_B64__);
  const chunkBytes = window.__CHUNK_BYTES__;

  for (let i = 0; i < window.__WARMUP__; i++) {
    term.reset();
    await runOnce(term, corpus, chunkBytes);
  }

  const throughputs: number[] = [];
  const frameP99s: number[] = [];
  for (let i = 0; i < window.__RUNS__; i++) {
    term.reset();
    const result = await runOnce(term, corpus, chunkBytes);
    throughputs.push(result.throughputMbS);
    frameP99s.push(result.frameP99Ms);
  }

  const throughput = summarize(throughputs);
  const report = {
    backend,
    frameP99MsMedian: summarize(frameP99s).median,
    runs: throughputs.length,
    throughputMbSMax: throughput.max,
    throughputMbSMedian: throughput.median,
    throughputMbSMin: throughput.min,
    totalBytes: corpus.length,
  };
  window.ipc?.postMessage(JSON.stringify(report));
}

run();
