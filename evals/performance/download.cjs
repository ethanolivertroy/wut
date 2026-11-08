'use strict';

// Synthetic transport benchmark: no provider keys, remote services or user data.
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const http = require('node:http');
const crypto = require('node:crypto');
const { spawnSync } = require('node:child_process');
const release = require('../../npm/lib/release.js');

async function main() {
  const root = fs.mkdtempSync(path.join(os.tmpdir(), 'wut-download-bench-'));
  const source = path.join(root, 'source');
  fs.mkdirSync(source);
  fs.writeFileSync(path.join(source, 'wut'), `#!/bin/sh\necho ${release.VERSION}\n`);
  fs.chmodSync(path.join(source, 'wut'), 0o755);
  const asset = release.assetFor();
  const archivePath = path.join(root, asset);
  const packed = spawnSync('tar', ['-czf', archivePath, '-C', source, 'wut']);
  if (packed.status !== 0) throw new Error('could not prepare synthetic archive');
  const archive = fs.readFileSync(archivePath);
  const checksum = crypto.createHash('sha256').update(archive).digest('hex') + `  ${asset}\n`;
  const delayMs = 100;
  const server = http.createServer((request, response) => {
    setTimeout(() => response.end(request.url.endsWith('.sha256') ? checksum : archive), delayMs);
  });
  await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
  process.env.WUT_NPM_BASE_URL = `http://127.0.0.1:${server.address().port}`;
  const samples = [];
  try {
    for (let index = 0; index < 6; index++) {
      process.env.WUT_NPM_CACHE_DIR = path.join(root, `cache-${index}`);
      const start = performance.now();
      await release.downloadBinary();
      if (index) samples.push(performance.now() - start);
    }
  } finally {
    server.closeAllConnections();
    await new Promise(resolve => server.close(resolve));
    fs.rmSync(root, { recursive: true, force: true });
  }
  const sorted = [...samples].sort((left, right) => left - right);
  console.log(JSON.stringify({
    case: 'cold_cache_archive_download', artificial_response_delay_ms: delayMs,
    samples_ms: samples, median_ms: sorted[2],
    scope: 'Five local synthetic downloads after one warmup. Includes 100 ms added per response; not a real network speed estimate.',
  }, null, 2));
}

main().catch(error => { console.error(error.message); process.exitCode = 1; });
