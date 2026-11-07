"""Ed25519 (RFC 8032 §6 reference code, pure Python): only for the CI
formats vectors (tests/release/fixtures/ci-formats-v1.json). Slow and not
constant-time: test vectors only, never a real key. It is a third,
independent implementation next to aws-lc-rs (k2-node-proto) and OpenSSL."""

import hashlib

p = 2 ** 255 - 19
q = 2 ** 252 + 27742317777372353535851937790883648493


def _sha512(m):
    return hashlib.sha512(m).digest()


def _inv(x):
    return pow(x, p - 2, p)


d = -121665 * _inv(121666) % p
_SQRT_M1 = pow(2, (p - 1) // 4, p)


def _add(P, Q):
    A, B = (P[1] - P[0]) * (Q[1] - Q[0]) % p, (P[1] + P[0]) * (Q[1] + Q[0]) % p
    C, D = 2 * P[3] * Q[3] * d % p, 2 * P[2] * Q[2] % p
    E, F, G, H = B - A, D - C, D + C, B + A
    return (E * F, G * H, F * G, E * H)


def _mul(s, P):
    Q = (0, 1, 1, 0)
    while s > 0:
        if s & 1:
            Q = _add(Q, P)
        P = _add(P, P)
        s >>= 1
    return Q


def _equal(P, Q):
    return (P[0] * Q[2] - Q[0] * P[2]) % p == 0 and (P[1] * Q[2] - Q[1] * P[2]) % p == 0


def _recover_x(y, sign):
    if y >= p:
        return None
    x2 = (y * y - 1) * _inv(d * y * y + 1)
    if x2 == 0:
        return None if sign else 0
    x = pow(x2, (p + 3) // 8, p)
    if (x * x - x2) % p != 0:
        x = x * _SQRT_M1 % p
    if (x * x - x2) % p != 0:
        return None
    if (x & 1) != sign:
        x = p - x
    return x


_GY = 4 * _inv(5) % p
_GX = _recover_x(_GY, 0)
_G = (_GX, _GY, 1, _GX * _GY % p)


def _compress(P):
    zinv = _inv(P[2])
    x, y = P[0] * zinv % p, P[1] * zinv % p
    return int.to_bytes(y | ((x & 1) << 255), 32, "little")


def _decompress(s):
    if len(s) != 32:
        return None
    y = int.from_bytes(s, "little")
    sign = y >> 255
    y &= (1 << 255) - 1
    x = _recover_x(y, sign)
    if x is None:
        return None
    return (x, y, 1, x * y % p)


def _expand(seed):
    h = _sha512(seed)
    a = int.from_bytes(h[:32], "little")
    a &= (1 << 254) - 8
    a |= 1 << 254
    return a, h[32:]


def _modq(s):
    return int.from_bytes(_sha512(s), "little") % q


def public_key(seed):
    """The 32-byte public key of a 32-byte seed."""
    a, _ = _expand(seed)
    return _compress(_mul(a, _G))


def sign(seed, msg):
    """The 64-byte signature (deterministic)."""
    a, prefix = _expand(seed)
    A = _compress(_mul(a, _G))
    r = _modq(prefix + msg)
    Rs = _compress(_mul(r, _G))
    s = (r + _modq(Rs + A + msg) * a) % q
    return Rs + int.to_bytes(s, 32, "little")


def verify(public, msg, sig):
    if len(public) != 32 or len(sig) != 64:
        return False
    A = _decompress(public)
    R = _decompress(sig[:32])
    if A is None or R is None:
        return False
    s = int.from_bytes(sig[32:], "little")
    if s >= q:
        return False
    h = _modq(sig[:32] + public + msg)
    return _equal(_mul(s, _G), _add(R, _mul(h, A)))
