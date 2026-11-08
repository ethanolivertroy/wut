'use strict';

const assert = require('node:assert/strict');
const crypto = require('node:crypto');
const fs = require('node:fs');
const http = require('node:http');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');
const { after, before, beforeEach, test } = require('node:test');

const release = require('../lib/release.js');

// The asset this machine would download, so the tests pass on macOS and Linux.
const ASSET = release.assetFor();
const VERSION_TEXT = require('../package.json').version;

let server;
let origin;
let served;
let workspace;
let respond;

function tarUp(contents, destination) {
  const source = fs.mkdtempSync(path.join(workspace, 'src-'));
  for (const [name, body] of Object.entries(contents)) {
    const file = path.join(source, name);
    fs.writeFileSync(file, body);
    fs.chmodSync(file, 0o755);
  }
  const result = spawnSync('tar', ['-czf', destination, '-C', source, ...Object.keys(contents)]);
  assert.equal(result.status, 0, `tar failed: ${result.stderr}`);
}

before(async () => {
  workspace = fs.mkdtempSync(path.join(os.tmpdir(), 'wut-npm-test-'));
  served = fs.mkdtempSync(path.join(workspace, 'served-'));

  server = http.createServer((request, response) => {
    if (respond) return respond(request, response);
    const name = path.basename(request.url.split('?')[0]);
    const file = path.join(served, name);
    if (!fs.existsSync(file)) {
      response.writeHead(404).end('not found');
      return;
    }
    response.writeHead(200, { 'content-type': 'application/octet-stream' });
    fs.createReadStream(file).pipe(response);
  });

  await new Promise((resolve) => server.listen(0, '127.0.0.1', resolve));
  origin = `http://127.0.0.1:${server.address().port}`;
  process.env.WUT_NPM_BASE_URL = origin;
});

after(() => {
  server.closeAllConnections();
  server.close();
  fs.rmSync(workspace, { recursive: true, force: true });
});

beforeEach(() => {
  respond = null;
  for (const name of fs.readdirSync(served)) {
    fs.rmSync(path.join(served, name), { force: true });
  }
  process.env.WUT_NPM_CACHE_DIR = fs.mkdtempSync(path.join(workspace, 'cache-'));
});

function publishRelease({ tamper = false } = {}) {
  const archive = path.join(workspace, `${ASSET}.${crypto.randomUUID()}`);
  tarUp({ wut: `#!/bin/sh\necho ${VERSION_TEXT}\n` }, archive);

  // The published checksum always describes the pristine archive, so a tampered
  // download must be rejected before anything is unpacked.
  const pristine = fs.readFileSync(archive);
  const hash = crypto.createHash('sha256').update(pristine).digest('hex');
  const bytes = tamper ? Buffer.concat([pristine, Buffer.from('tampered')]) : pristine;

  fs.writeFileSync(path.join(served, ASSET), bytes);
  fs.writeFileSync(path.join(served, `${ASSET}.sha256`), `${hash}  ${ASSET}\n`);
  return hash;
}

test('maps platforms to release assets', () => {
  assert.equal(release.assetFor('darwin', 'arm64'), 'wut-aarch64-apple-darwin.tar.gz');
  assert.equal(release.assetFor('darwin', 'x64'), 'wut-x86_64-apple-darwin.tar.gz');
  assert.equal(release.assetFor('linux', 'arm64'), 'wut-aarch64-unknown-linux-musl.tar.gz');
  assert.equal(release.assetFor('linux', 'x64'), 'wut-x86_64-unknown-linux-musl.tar.gz');
  assert.throws(() => release.assetFor('win32', 'x64'), /unsupported platform/);
  assert.throws(() => release.assetFor('linux', 'ia32'), /unsupported platform/);
});

test('parses and validates checksum files', () => {
  const hash = 'a'.repeat(64);
  assert.equal(release.expectedChecksum(`${hash}  ${ASSET}\n`, ASSET), hash);
  assert.equal(release.expectedChecksum(`${hash} *${ASSET}\n`, ASSET), hash);
  assert.equal(release.expectedChecksum(`${hash}\n`, ASSET), hash);
  assert.throws(() => release.expectedChecksum('nonsense', ASSET), /malformed checksum/);
  assert.throws(() => release.expectedChecksum(`${hash}  other.tar.gz`, ASSET), /names other/);
});

test('downloads, verifies, unpacks, and runs the binary', async () => {
  const published = publishRelease();
  const binary = await release.ensureBinary();

  assert.equal(published.length, 64);
  assert.ok(fs.existsSync(binary), `missing binary at ${binary}`);
  assert.ok(fs.statSync(binary).mode & 0o100, 'binary is not executable');
  assert.deepEqual(fs.readdirSync(path.dirname(binary)), ['wut'], 'cache should hold only the binary');
  assert.match(fs.readFileSync(binary, 'utf8'), new RegExp(VERSION_TEXT));

  const run = spawnSync(binary, ['--version'], { encoding: 'utf8' });
  assert.equal(run.status, 0, run.stderr);
  assert.equal(run.stdout.trim(), VERSION_TEXT);
});

test('reuses the cached binary without re-downloading', async () => {
  publishRelease();
  const first = await release.ensureBinary();

  for (const name of fs.readdirSync(served)) {
    fs.rmSync(path.join(served, name), { force: true });
  }

  const second = await release.ensureBinary();
  assert.equal(second, first);
  assert.equal(spawnSync(second, ['--version'], { encoding: 'utf8' }).stdout.trim(), VERSION_TEXT);
});

test('refuses a tampered archive and installs nothing', async () => {
  const published = publishRelease({ tamper: true });

  await assert.rejects(release.ensureBinary(), (error) => {
    assert.match(error.message, /checksum mismatch/);
    assert.match(error.message, new RegExp(published.slice(0, 12)));
    return true;
  });

  assert.equal(
    fs.existsSync(release.binaryPath()),
    false,
    'a rejected download must not leave a binary behind',
  );
});

test('starts archive and checksum requests before either response completes', async () => {
  publishRelease();
  const pending = [];
  respond = (request, response) => {
    pending.push([request, response]);
    if (pending.length === 2) {
      for (const [req, res] of pending) {
        res.end(fs.readFileSync(path.join(served, path.basename(req.url))));
      }
    }
  };
  await release.downloadBinary({ timeoutMs: 2000 });
  assert.equal(pending.length, 2);
});

test('bounds downloads stalled before headers and during a response body', async () => {
  for (const startedBody of [false, true]) {
    let bodyStarted = false;
    respond = (_request, response) => {
      if (startedBody) {
        bodyStarted = true;
        response.writeHead(200);
        response.write('partial body');
      }
    };
    await assert.rejects(release.downloadBinary({ timeoutMs: 200 }), /download timed out/);
    if (startedBody) assert.equal(bodyStarted, true, 'response body never started');
    assert.equal(fs.existsSync(release.binaryPath()), false);
  }
});

test('rejects advertised oversized archives and cancels the pending checksum', async () => {
  const pending = [];
  let peerClosed;
  const closed = new Promise(resolve => { peerClosed = resolve; });
  respond = (request, response) => {
    pending.push([request, response]);
    if (pending.length === 2) {
      for (const [req, res] of pending) {
        if (req.url.endsWith('.sha256')) {
          res.on('close', peerClosed);
          res.write('unfinished checksum');
        } else {
          res.writeHead(200, { 'content-length': '2048' });
          res.flushHeaders();
        }
      }
    }
  };
  await assert.rejects(release.downloadBinary({ maxArchiveBytes: 1024 }), /exceeds 1024 bytes/);
  const timer = setTimeout(() => peerClosed('timeout'), 2000);
  try {
    assert.notEqual(await closed, 'timeout', 'companion download was not cancelled');
  } finally {
    clearTimeout(timer);
  }
  assert.equal(fs.existsSync(release.binaryPath()), false);
});

test('bounds chunked archive and checksum bodies without content-length', async () => {
  for (const oversizedChecksum of [false, true]) {
    publishRelease();
    respond = (request, response) => {
      if (request.url.endsWith('.sha256') === oversizedChecksum) {
        response.write(Buffer.alloc(3000));
        response.end(Buffer.alloc(3000));
      } else {
        response.end(fs.readFileSync(path.join(served, path.basename(request.url))));
      }
    };
    const limit = oversizedChecksum ? 4096 : 1024;
    await assert.rejects(release.downloadBinary({ maxArchiveBytes: 1024 }),
      new RegExp(`exceeds ${limit} bytes`));
    assert.equal(fs.existsSync(release.binaryPath()), false);
  }
});

test('a failed download preserves an existing cached binary', async () => {
  publishRelease();
  const binary = await release.ensureBinary();
  const before = fs.readFileSync(binary);
  respond = (_request, response) => response.writeHead(503).end('unavailable');
  await assert.rejects(release.downloadBinary(), /HTTP 503/);
  assert.deepEqual(fs.readFileSync(binary), before);
  assert.deepEqual(fs.readdirSync(process.env.WUT_NPM_CACHE_DIR), [VERSION_TEXT]);
});
