/**
 * Unit tests for edge loader version rules (no browser / Caddy needed).
 *
 * Run:  bun test web/loader/loader.test.js
 *   or: node --test web/loader/loader.test.js  (Node 18+)
 */
'use strict';

const { describe, it } = require('node:test');
const assert = require('node:assert/strict');
const {
  isValidVersion,
  parseSemver,
  compareSemverCore,
  isAtLeast,
  pickVersionField,
  acceptVersion,
  isPublicView,
  pickPointerVersion,
  resolveVersion,
  MIN_SUPPORT_VERSION,
} = require('./loader.js');

// PRD boot-status-ghost §3.1 — a server in ghost mode answers a signed-out
// browser without `version`; the loader takes /app/current.json instead.
describe('boot-status views', () => {
  function stubBootStatus(body) {
    const prev = globalThis.fetch;
    const urls = [];
    globalThis.fetch = async (url) => {
      urls.push(url);
      if (url === '/boot-status') return { ok: true, status: 200, json: async () => body };
      throw new Error('unexpected fetch ' + url);
    };
    return { urls, restore: () => { globalThis.fetch = prev; } };
  }

  it('isPublicView only for view:"public"', () => {
    assert.equal(isPublicView({ view: 'public', phase: 'ready' }), true);
    assert.equal(isPublicView({ view: 'compat', version: '0.46.3' }), false);
    assert.equal(isPublicView({ version: '0.46.2' }), false);
    assert.equal(isPublicView(null), false);
  });

  it('pickPointerVersion checks the protocol range', () => {
    const st = { view: 'public', protocol: 1, protocolMin: 1 };
    assert.deepEqual(pickPointerVersion(st, { version: '0.46.3', protocol: 1 }), { ok: true, version: '0.46.3' });
    assert.equal(pickPointerVersion({ ...st, protocol: 3, protocolMin: 2 }, { version: '0.46.3', protocol: 1 }).reason, 'below_floor');
    assert.equal(pickPointerVersion(st, null).reason, 'boot_parse');
    assert.equal(pickPointerVersion(st, { protocol: 1 }).reason, 'boot_parse');
  });

  it('a compat / full / old body keeps the version path (no pointer fetch)', async () => {
    for (const body of [
      { view: 'compat', phase: 'ready', version: '0.46.3' },
      { view: 'full', phase: 'ready', version: '0.46.3' },
      { phase: 'ready', version: '0.46.2' },
    ]) {
      const s = stubBootStatus(body);
      try {
        const v = await resolveVersion({
          override: null,
          fetchCurrentPointer: async () => { throw new Error('pointer must not be read'); },
        });
        assert.equal(v, body.version);
        assert.deepEqual(s.urls, ['/boot-status']);
      } finally {
        s.restore();
      }
    }
  });

  it('a public body loads the published current bundle', async () => {
    const s = stubBootStatus({ view: 'public', phase: 'ready', protocol: 1, protocolMin: 1, instanceId: 'x' });
    try {
      const v = await resolveVersion({ override: null, fetchCurrentPointer: async () => ({ version: '0.46.3', protocol: 1 }) });
      assert.equal(v, '0.46.3');
    } finally {
      s.restore();
    }
  });

  it('a public body with no pointer / a bad pointer fails loudly, never loads junk', async () => {
    const s = stubBootStatus({ view: 'public', phase: 'ready', protocol: 1, protocolMin: 1, instanceId: 'x' });
    try {
      await assert.rejects(
        resolveVersion({ override: null, fetchCurrentPointer: async () => ({ version: '../evil', protocol: 1 }) }),
        (e) => e.code === 'malformed',
      );
      await assert.rejects(
        resolveVersion({ override: null, fetchCurrentPointer: async () => ({ version: '0.46.3', protocol: 9 }) }),
        (e) => e.code === 'below_floor',
      );
      await assert.rejects(
        resolveVersion({ override: null, fetchCurrentPointer: async () => null }),
        (e) => e.code === 'boot_parse',
      );
    } finally {
      s.restore();
    }
  });
});

describe('isValidVersion', () => {
  it('accepts plain and pre-release semver-ish', () => {
    assert.equal(isValidVersion('0.40.53'), true);
    assert.equal(isValidVersion('1.0.0'), true);
    assert.equal(isValidVersion('0.40.53-rc.1'), true);
    assert.equal(isValidVersion('10.2.3+build.9'), true);
  });

  it('rejects empty / non-string', () => {
    assert.equal(isValidVersion(''), false);
    assert.equal(isValidVersion('   '), false);
    assert.equal(isValidVersion(null), false);
    assert.equal(isValidVersion(undefined), false);
    assert.equal(isValidVersion(40), false);
  });

  it('rejects path injection and traversal', () => {
    assert.equal(isValidVersion('../etc/passwd'), false);
    assert.equal(isValidVersion('0.40.53/../0.1.0'), false);
    assert.equal(isValidVersion('0.40.53/evil'), false);
    assert.equal(isValidVersion('..'), false);
    assert.equal(isValidVersion('0.40.53\\x'), false);
    assert.equal(isValidVersion('%2e%2e'), false);
    assert.equal(isValidVersion('/0.40.53'), false);
  });

  it('rejects other malformed tokens', () => {
    assert.equal(isValidVersion('v0.40.53'), false);
    assert.equal(isValidVersion('0.40'), false);
    assert.equal(isValidVersion('latest'), false);
    assert.equal(isValidVersion('0.40.53 script'), false);
  });
});

describe('compareSemverCore / isAtLeast', () => {
  it('orders major.minor.patch', () => {
    assert.equal(compareSemverCore('0.40.53', '0.40.52'), 1);
    assert.equal(compareSemverCore('0.40.0', '0.40.0'), 0);
    assert.equal(compareSemverCore('0.39.99', '0.40.0'), -1);
    assert.equal(compareSemverCore('1.0.0', '0.99.99'), 1);
  });

  it('support floor accepts current and rejects below', () => {
    assert.equal(isAtLeast('0.40.0', MIN_SUPPORT_VERSION), true);
    assert.equal(isAtLeast('0.40.53', MIN_SUPPORT_VERSION), true);
    assert.equal(isAtLeast('0.39.9', MIN_SUPPORT_VERSION), false);
  });
});

describe('pickVersionField', () => {
  it('prefers webClientVersion when non-empty', () => {
    assert.equal(
      pickVersionField({ version: '0.40.1', webClientVersion: '0.40.53' }),
      '0.40.53',
    );
  });

  it('falls back to version', () => {
    assert.equal(pickVersionField({ version: '0.40.53' }), '0.40.53');
    assert.equal(pickVersionField({ version: '0.40.53', webClientVersion: '' }), '0.40.53');
  });

  it('returns null when neither is usable', () => {
    assert.equal(pickVersionField({}), null);
    assert.equal(pickVersionField(null), null);
    assert.equal(pickVersionField({ webClientVersion: '  ' }), null);
  });
});

describe('acceptVersion', () => {
  it('ok for supported versions', () => {
    const r = acceptVersion('0.40.53');
    assert.equal(r.ok, true);
    assert.equal(r.version, '0.40.53');
  });

  it('malformed for injection / empty', () => {
    assert.equal(acceptVersion('').ok, false);
    assert.equal(acceptVersion('../x').reason, 'malformed');
    assert.equal(acceptVersion('latest').reason, 'malformed');
  });

  it('below_floor for old daemons', () => {
    const r = acceptVersion('0.30.0');
    assert.equal(r.ok, false);
    assert.equal(r.reason, 'below_floor');
  });
});

describe('parseSemver', () => {
  it('parses core + suffix', () => {
    assert.deepEqual(parseSemver('0.40.53-rc.1'), {
      major: 0,
      minor: 40,
      patch: 53,
      pre: '-rc.1',
    });
  });
});
