#!/usr/bin/env node
'use strict';

const { spawn } = require('node:child_process');
const { ensureBinary } = require('../lib/release.js');

async function main() {
  let binary;
  try {
    binary = await ensureBinary();
  } catch (error) {
    process.stderr.write(`wut: ${error.message}\n`);
    if (error.hint) {
      process.stderr.write(`     ${error.hint}\n`);
    }
    process.exitCode = 1;
    return;
  }

  const child = spawn(binary, process.argv.slice(2), { stdio: 'inherit' });

  const forwarding = new Map();
  for (const signal of ['SIGINT', 'SIGTERM', 'SIGHUP']) {
    const forward = () => {
      // killed means a signal was sent, not that the child has exited.
      if (child.exitCode === null && child.signalCode === null) {
        child.kill(signal);
      }
    };
    forwarding.set(signal, forward);
    process.on(signal, forward);
  }

  const stopForwarding = () => {
    for (const [signal, forward] of forwarding) {
      process.removeListener(signal, forward);
    }
  };

  child.on('error', (error) => {
    stopForwarding();
    process.stderr.write(`wut: could not start ${binary}: ${error.message}\n`);
    process.exitCode = 1;
  });

  child.on('exit', (code, signal) => {
    stopForwarding();
    if (signal) {
      process.kill(process.pid, signal);
    } else {
      process.exitCode = code === null ? 1 : code;
    }
  });
}

main();
