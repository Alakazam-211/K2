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
  MIN_SUPPORT_VERSION,
} = require('./loader.js');

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
