'use strict';

const crypto = require('node:crypto');
const fs = require('node:fs');
const os = require('node:os');
const path = require('node:path');
const { spawnSync } = require('node:child_process');

const REPOSITORY = 'ethanolivertroy/wut';
const VERSION = require('../package.json').version;
const DOWNLOAD_TIMEOUT_MS = 60_000;
const MAX_ARCHIVE_BYTES = 32 * 1024 * 1024;
const MAX_CHECKSUM_BYTES = 4096;

const INSTALL_HINT =
  `install the standalone binary instead:\n` +
  `       curl -fsSL https://raw.githubusercontent.com/${REPOSITORY}/main/install.sh | sh`;

const ASSETS = {
  'darwin arm64': 'wut-aarch64-apple-darwin.tar.gz',
  'darwin x64': 'wut-x86_64-apple-darwin.tar.gz',
  'linux arm64': 'wut-aarch64-unknown-linux-musl.tar.gz',
  'linux x64': 'wut-x86_64-unknown-linux-musl.tar.gz',
};

// Which release asset belongs on this machine.
function assetFor(platform = process.platform, arch = process.arch) {
  const asset = ASSETS[`${platform} ${arch}`];
  if (!asset) {
    const error = new Error(`unsupported platform: ${platform}/${arch}`);
    error.hint = INSTALL_HINT;
    throw error;
  }
  return asset;
}

function releaseBase(version = VERSION) {
  return process.env.WUT_NPM_BASE_URL
    ? process.env.WUT_NPM_BASE_URL.replace(/\/+$/, '')
    : `https://github.com/${REPOSITORY}/releases/download/v${version}`;
}

function cacheRoot() {
  const base =
    process.env.WUT_NPM_CACHE_DIR ||
    path.join(process.env.XDG_CACHE_HOME || path.join(os.homedir(), '.cache'), 'wut-npm');
  return base;
}

function binaryPath(version = VERSION, target = cacheRoot()) {
  return path.join(target, version, process.platform === 'win32' ? 'wut.exe' : 'wut');
}

function sha256(buffer) {
  return crypto.createHash('sha256').update(buffer).digest('hex');
}

// Parse a release `.sha256` file: "<64 hex>  <filename>".
function expectedChecksum(text, asset) {
  const [hash, name] = String(text).trim().split(/\s+/);
  if (!/^[0-9a-f]{64}$/.test(hash || '')) {
    throw new Error(`malformed checksum file for ${asset}`);
  }
  if (name && name.replace(/^\*/, '') !== asset) {
    throw new Error(`checksum file names ${name}, expected ${asset}`);
  }
  return hash;
}

async function fetchBytes(url, limit, signal) {
  const response = await fetch(url, { redirect: 'follow', signal });
  if (!response.ok) {
    throw new Error(`could not download ${url} (HTTP ${response.status})`);
  }
  const tooLarge = () => new Error(`download exceeds ${limit} bytes: ${url}`);
  if (Number(response.headers.get('content-length')) > limit) {
    throw tooLarge();
  }
  if (!response.body) {
    throw new Error(`empty download response: ${url}`);
  }
  const reader = response.body.getReader();
  const chunks = [];
  let length = 0;
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      if (value.byteLength > limit - length) throw tooLarge();
      chunks.push(Buffer.from(value));
      length += value.byteLength;
    }
  } finally {
    reader.releaseLock();
  }
  return Buffer.concat(chunks, length);
}

function requireTar() {
  const probe = spawnSync('tar', ['--version'], { stdio: 'ignore' });
  if (probe.error) {
    const error = new Error('the tar command is required to unpack wut');
    error.hint = INSTALL_HINT;
    throw error;
  }
}

async function downloadBinary({
  version = VERSION, asset = assetFor(),
  timeoutMs = DOWNLOAD_TIMEOUT_MS, maxArchiveBytes = MAX_ARCHIVE_BYTES,
} = {}) {
  const base = releaseBase(version);
  const controller = new AbortController();
  let timedOut = false;
  const timer = setTimeout(() => {
    timedOut = true;
    controller.abort();
  }, timeoutMs);
  let archive, checksum;
  try {
    [archive, checksum] = await Promise.all([
      fetchBytes(`${base}/${asset}`, maxArchiveBytes, controller.signal),
      fetchBytes(`${base}/${asset}.sha256`, MAX_CHECKSUM_BYTES, controller.signal),
    ]);
  } catch (error) {
    // Stop the companion request as well; no partial download enters the cache.
    controller.abort();
    if (timedOut) throw new Error(`download timed out after ${timeoutMs} ms`);
    throw error;
  } finally {
    clearTimeout(timer);
  }
  const published = expectedChecksum(checksum, asset);
  const actual = sha256(archive);

  if (actual !== published) {
    throw new Error(
      `checksum mismatch for ${asset} (expected ${published}, downloaded ${actual}); refusing to install`,
    );
  }

  requireTar();
  fs.mkdirSync(cacheRoot(), { recursive: true });
  const staging = fs.mkdtempSync(path.join(cacheRoot(), `.staging-${version}-`));
  const target = path.join(cacheRoot(), version);

  try {
    const archivePath = path.join(staging, asset);
    fs.writeFileSync(archivePath, archive);
    const unpacked = path.join(staging, 'unpacked');
    fs.mkdirSync(unpacked);

    const extracted = spawnSync('tar', ['-xzf', archivePath, '-C', unpacked], { stdio: 'inherit' });
    if (extracted.status !== 0) {
      throw new Error(`could not unpack ${asset}`);
    }

    const binary = path.join(unpacked, 'wut');
    if (!fs.existsSync(binary)) {
      throw new Error(`${asset} does not contain a wut binary`);
    }

    // Stage only the binary, then move it into place atomically.
    const staged = path.join(staging, version);
    fs.mkdirSync(staged);
    fs.copyFileSync(binary, path.join(staged, 'wut'));
    fs.chmodSync(path.join(staged, 'wut'), 0o755);

    if (fs.existsSync(target)) {
      fs.rmSync(target, { recursive: true, force: true });
    }
    fs.renameSync(staged, target);
  } finally {
    fs.rmSync(staging, { recursive: true, force: true });
  }

  if (process.env.WUT_NPM_VERBOSE) {
    process.stderr.write(`wut: installed ${version} to ${target}\n`);
  }
  return target;
}

// Download the binary once, then reuse it from the cache.
async function ensureBinary({ version = VERSION, force = false, target = cacheRoot() } = {}) {
  const binary = binaryPath(version, target);
  if (!force && fs.existsSync(binary)) {
    return binary;
  }
  await downloadBinary({ version });
  if (!fs.existsSync(binary)) {
    throw new Error(`wut ${version} was not installed to ${binary}`);
  }
  return binary;
}

module.exports = {
  ASSETS,
  VERSION,
  assetFor,
  binaryPath,
  cacheRoot,
  downloadBinary,
  ensureBinary,
  expectedChecksum,
  releaseBase,
  sha256,
};
