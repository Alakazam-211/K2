/**
 * Tiny edge loader for the hosted web client (PRD §2.2).
 *
 * 1. Optional ?v=<ver> override (same validation as boot-status).
 * 2. Else GET /boot-status → webClientVersion || version.
 * 3. Validate semver-ish (block path injection / "..").
 * 4. Optional support-floor check.
 * 5. HEAD /app/<ver>/index.html → navigate, or "build not found".
 * 6. Timeout/network fail → friendly "server unreachable" (no hung spinner).
 *
 * Browser: auto-boots on DOMContentLoaded.
 * Node/bun tests: module.exports the pure helpers.
 */
(function (root) {
  'use strict';

  /** Reject empty, path-ish, and non-semver-ish tokens. Allows pre/build (+ - .). */
  var VERSION_RE = /^\d+\.\d+\.\d+([a-zA-Z0-9.+-]*)?$/;

  /**
   * Oldest web bundle we will load. Daemons below this get
   * "update your server" rather than a known-vulnerable old SPA.
   * Keep in lockstep with ConnectionGate floor when that is published.
   */
  var MIN_SUPPORT_VERSION = '0.40.0';

  var BOOT_TIMEOUT_MS = 8000;
  var HEAD_TIMEOUT_MS = 5000;

  function isValidVersion(v) {
    if (typeof v !== 'string') return false;
    var s = v.trim();
    if (!s) return false;
    // Explicit path-injection guards (regex already blocks most).
    if (s.indexOf('..') !== -1) return false;
    if (s.indexOf('/') !== -1 || s.indexOf('\\') !== -1) return false;
    if (s.indexOf('%') !== -1) return false;
    return VERSION_RE.test(s);
  }

  /** Parse "1.2.3-pre.1+build" → { major, minor, patch, pre } or null. */
  function parseSemver(v) {
    if (!isValidVersion(v)) return null;
    var m = String(v).trim().match(/^(\d+)\.(\d+)\.(\d+)(.*)$/);
    if (!m) return null;
    return {
      major: +m[1],
      minor: +m[2],
      patch: +m[3],
      pre: m[4] || '',
    };
  }

  /**
   * Numeric core compare only (major.minor.patch).
   * Returns -1 / 0 / 1, or null if either side is invalid.
   * Prerelease suffix does not demote the core for the support floor
   * (0.40.0-rc.1 still clears a 0.40.0 floor numerically equal — floor
   * rejects only when core is strictly less).
   */
  function compareSemverCore(a, b) {
    var pa = parseSemver(a);
    var pb = parseSemver(b);
    if (!pa || !pb) return null;
    if (pa.major !== pb.major) return pa.major < pb.major ? -1 : 1;
    if (pa.minor !== pb.minor) return pa.minor < pb.minor ? -1 : 1;
    if (pa.patch !== pb.patch) return pa.patch < pb.patch ? -1 : 1;
    return 0;
  }

  function isAtLeast(version, floor) {
    var c = compareSemverCore(version, floor);
    return c !== null && c >= 0;
  }

  /** Prefer webClientVersion when present and non-empty; else version. */
  function pickVersionField(status) {
    if (!status || typeof status !== 'object') return null;
    var w = status.webClientVersion;
    if (typeof w === 'string' && w.trim()) return w.trim();
    var v = status.version;
    if (typeof v === 'string' && v.trim()) return v.trim();
    return null;
  }

  function setStatus(kind, title, detail, hint) {
    if (typeof document === 'undefined') return;
    var el = document.getElementById('status');
    var titleEl = document.getElementById('title');
    var detailEl = document.getElementById('detail');
    var hintEl = document.getElementById('hint');
    var spin = document.getElementById('spin');
    if (!el || !titleEl || !detailEl) return;
    el.className = 'status ' + (kind || '');
    titleEl.textContent = title || '';
    detailEl.textContent = detail || '';
    if (spin) {
      if (kind === 'loading') spin.classList.remove('hidden');
      else spin.classList.add('hidden');
    }
    if (hintEl) {
      if (hint) {
        hintEl.textContent = hint;
        hintEl.classList.remove('hidden');
      } else {
        hintEl.textContent = '';
        hintEl.classList.add('hidden');
      }
    }
  }

  function showUnreachable(reason) {
    setStatus(
      'error',
      'Server unreachable',
      reason ||
        'Could not reach this K2 server. It may be offline, asleep, or the tunnel may be down.',
      'Retry when the box is up, or check the tunnel / daemon.',
    );
  }

  function showBadVersion(msg) {
    setStatus(
      'error',
      'Update your server',
      msg || 'This server advertised an unsupported or invalid client version.',
      'Upgrade the K2 daemon, then reload.',
    );
  }

  function showBuildMissing(ver) {
    setStatus(
      'error',
      'Build not found',
      'No web client bundle is available for version ' + ver + '.',
      'Publish app/' + ver + '/ to the edge, or update the daemon to a published version.',
    );
  }

  function showMalformed(msg) {
    setStatus(
      'error',
      'Invalid version',
      msg || 'The server returned an empty or malformed version string.',
      'Support can force a bundle with ?v=x.y.z on this URL.',
    );
  }

  function queryOverride() {
    if (typeof location === 'undefined') return null;
    try {
      var params = new URLSearchParams(location.search || '');
      var v = params.get('v');
      return v && v.trim() ? v.trim() : null;
    } catch (_) {
      return null;
    }
  }

  function fetchBootStatus(timeoutMs) {
    var ms = timeoutMs == null ? BOOT_TIMEOUT_MS : timeoutMs;
    return fetch('/boot-status', {
      method: 'GET',
      credentials: 'same-origin',
      cache: 'no-store',
      signal: AbortSignal.timeout(ms),
    }).then(function (res) {
      if (!res.ok) {
        var err = new Error('boot-status HTTP ' + res.status);
        err.code = 'http';
        err.status = res.status;
        throw err;
      }
      return res.json();
    });
  }

  function headBundle(version, timeoutMs) {
    var ms = timeoutMs == null ? HEAD_TIMEOUT_MS : timeoutMs;
    var url = '/app/' + encodeURIComponent(version) + '/index.html';
    return fetch(url, {
      method: 'HEAD',
      credentials: 'same-origin',
      cache: 'no-store',
      signal: AbortSignal.timeout(ms),
    }).then(function (res) {
      // Some static servers omit HEAD; fall back to GET range-less if 405/501.
      if (res.status === 405 || res.status === 501) {
        return fetch(url, {
          method: 'GET',
          credentials: 'same-origin',
          cache: 'no-store',
          signal: AbortSignal.timeout(ms),
        });
      }
      return res;
    });
  }

  function navigateToApp(version) {
    // Land on index.html explicitly. Edge R2 is exact-key (no directory
    // index): `/app/<ver>/` 404s even when `/app/<ver>/index.html` exists.
    // Built assets use absolute `/app/<ver>/assets/...` paths (Vite base),
    // so index.html is safe and correct.
    var target = '/app/' + encodeURIComponent(version) + '/index.html';
    if (typeof location !== 'undefined') {
      location.replace(target);
    }
    return target;
  }

  /**
   * Validate a candidate version string against regex + support floor.
   * @returns {{ ok: true, version: string } | { ok: false, reason: 'malformed'|'below_floor', message: string }}
   */
  function acceptVersion(raw) {
    if (raw == null || (typeof raw === 'string' && !raw.trim())) {
      return {
        ok: false,
        reason: 'malformed',
        message: 'Version is empty.',
      };
    }
    var v = String(raw).trim();
    if (!isValidVersion(v)) {
      return {
        ok: false,
        reason: 'malformed',
        message: 'Version "' + v + '" is not a valid client version.',
      };
    }
    if (!isAtLeast(v, MIN_SUPPORT_VERSION)) {
      return {
        ok: false,
        reason: 'below_floor',
        message:
          'Version ' +
          v +
          ' is below the minimum supported client (' +
          MIN_SUPPORT_VERSION +
          ').',
      };
    }
    return { ok: true, version: v };
  }

  /**
   * Resolve the SPA version: ?v= override wins; else boot-status.
   * Does not navigate — returns the accepted version string or throws
   * an Error with .code in: override_bad | boot_network | boot_http |
   * boot_parse | malformed | below_floor.
   */
  function resolveVersion(opts) {
    opts = opts || {};
    var override =
      opts.override !== undefined ? opts.override : queryOverride();

    if (override != null && override !== '') {
      var ov = acceptVersion(override);
      if (!ov.ok) {
        var oerr = new Error(ov.message);
        oerr.code = ov.reason === 'below_floor' ? 'below_floor' : 'override_bad';
        return Promise.reject(oerr);
      }
      return Promise.resolve(ov.version);
    }

    var timeout = opts.bootTimeoutMs != null ? opts.bootTimeoutMs : BOOT_TIMEOUT_MS;
    return fetchBootStatus(timeout)
      .catch(function (e) {
        var err = new Error(
          (e && e.name === 'TimeoutError') ||
            (e && e.name === 'AbortError')
            ? 'Timed out waiting for /boot-status.'
            : 'Network error reaching /boot-status.',
        );
        err.code = 'boot_network';
        err.cause = e;
        throw err;
      })
      .then(function (status) {
        var picked = pickVersionField(status);
        if (picked == null) {
          var perr = new Error(
            'boot-status had no usable version or webClientVersion field.',
          );
          perr.code = 'boot_parse';
          throw perr;
        }
        var acc = acceptVersion(picked);
        if (!acc.ok) {
          var aerr = new Error(acc.message);
          aerr.code = acc.reason === 'below_floor' ? 'below_floor' : 'malformed';
          throw aerr;
        }
        return acc.version;
      });
  }

  /**
   * Full boot path for the browser.
   */
  function applyPageTitle() {
    try {
      if (typeof document === 'undefined' || typeof location === 'undefined') return;
      var h = String(location.hostname || '').toLowerCase();
      if (!h || /^\d+\.\d+\.\d+\.\d+$/.test(h)) return;
      var sub = h.split('.')[0];
      if (sub) document.title = sub + ' | K2';
    } catch (_e) {
      /* ignore */
    }
  }

  function boot(opts) {
    opts = opts || {};
    applyPageTitle();
    setStatus('loading', 'Connecting…', 'Looking up your server version.');

    return resolveVersion(opts)
      .then(function (version) {
        setStatus(
          'loading',
          'Loading client…',
          'Starting web client ' + version + '.',
        );
        return headBundle(version, opts.headTimeoutMs)
          .then(function (res) {
            if (!res.ok) {
              var err = new Error('Bundle missing for ' + version);
              err.code = 'build_missing';
              err.version = version;
              err.status = res.status;
              throw err;
            }
            navigateToApp(version);
            return version;
          })
          .catch(function (e) {
            if (e && e.code === 'build_missing') throw e;
            // HEAD network failure is ambiguous (edge down vs missing).
            // Still try navigate — if the bundle is there, SPA loads;
            // if not, user sees edge 404. Prefer explicit missing when
            // we got an HTTP error above.
            if (e && (e.name === 'TimeoutError' || e.name === 'AbortError' || e.name === 'TypeError')) {
              // Soft-fail HEAD: proceed to navigate; SPA load will fail visibly if missing.
              navigateToApp(version);
              return version;
            }
            throw e;
          });
      })
      .catch(function (e) {
        var code = (e && e.code) || '';
        if (code === 'boot_network' || code === 'boot_http') {
          showUnreachable(e.message);
        } else if (code === 'below_floor') {
          showBadVersion(e.message);
        } else if (code === 'build_missing') {
          showBuildMissing(e.version || '?');
        } else if (
          code === 'malformed' ||
          code === 'override_bad' ||
          code === 'boot_parse'
        ) {
          showMalformed(e.message);
        } else {
          showUnreachable(
            (e && e.message) || 'Could not start the web client.',
          );
        }
        return null;
      });
  }

  var api = {
    VERSION_RE: VERSION_RE,
    MIN_SUPPORT_VERSION: MIN_SUPPORT_VERSION,
    isValidVersion: isValidVersion,
    parseSemver: parseSemver,
    compareSemverCore: compareSemverCore,
    isAtLeast: isAtLeast,
    pickVersionField: pickVersionField,
    acceptVersion: acceptVersion,
    resolveVersion: resolveVersion,
    navigateToApp: navigateToApp,
    boot: boot,
    setStatus: setStatus,
  };

  // CommonJS for node/bun unit tests.
  if (typeof module !== 'undefined' && module.exports) {
    module.exports = api;
  }

  // Browser global + auto-boot.
  if (typeof window !== 'undefined') {
    root.K2Loader = api;
    if (document.readyState === 'loading') {
      document.addEventListener('DOMContentLoaded', function () {
        api.boot();
      });
    } else {
      api.boot();
    }
  }
})(typeof globalThis !== 'undefined' ? globalThis : this);
