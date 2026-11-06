"""Self-tests of the fake music servers (tools/esphome-fake-ha/fake_proxy.py, milestone M1).

Stdlib only; the ffprobe/ffmpeg checks are skipped where ffmpeg is missing
or WEBOS_SAT_SLOW_TESTS=1 is not set, and the HTTPS check where openssl is
missing. No network beyond 127.0.0.1.

Run: python3 -m unittest discover -s tools -p 'test_fake_proxy.py'
With the ffmpeg checks: WEBOS_SAT_SLOW_TESTS=1 python3 -m unittest discover -s tools -p 'test_fake_proxy.py'
"""
import asyncio
import os
import subprocess
import sys
import tempfile
import time
import unittest

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "esphome-fake-ha"))
import fake_proxy as fp  # noqa: E402

HAVE_FFMPEG = fp.ffmpeg_available()
HAVE_OPENSSL = subprocess.run(["which", "openssl"], capture_output=True).returncode == 0
# The real-ffmpeg classes take about 50 s of one core (30 s signals generated
# in Python and converted by ffmpeg): opt-in, like the other slow tests
# (docs/development.md#tests). CI sets it.
SLOW = os.environ.get("WEBOS_SAT_SLOW_TESTS") == "1"
SLOW_REASON = "slow: real ffmpeg conversions, about 50 s; set WEBOS_SAT_SLOW_TESTS=1"
SHORT = fp.Signal((fp.Track(0.5, 440.0),), rate=44100, channels=2)  # ~88 KB of PCM
LONG = fp.Signal((fp.Track(30.0, 440.0),), rate=44100, channels=2)  # never fits in the socket buffers


Fmt = fp.AdvertisedFormat


SAT_FORMATS = [Fmt("flac", 22050, 1, fp.PURPOSE_ANNOUNCEMENT), Fmt("flac", 48000, 2, fp.PURPOSE_DEFAULT)]


async def until(pred, timeout=5.0):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        if pred():
            return True
        await asyncio.sleep(0.02)
    return pred()


async def open_stream(url):
    """Sends the request and reads the response head; the caller reads the body."""
    from urllib.parse import urlsplit

    u = urlsplit(url)
    r, w = await asyncio.open_connection(u.hostname, u.port)
    w.write(f"GET {u.path} HTTP/1.1\r\nHost: x\r\n\r\n".encode())
    await w.drain()
    head = await r.readuntil(b"\r\n\r\n")
    return r, w, head


def ffprobe(data: bytes, suffix: str) -> dict:
    with tempfile.NamedTemporaryFile(suffix=suffix) as f:
        f.write(data)
        f.flush()
        out = subprocess.run(
            ["ffprobe", "-v", "error", "-select_streams", "a:0", "-show_entries",
             "stream=codec_name,sample_rate,channels", "-of", "default=nw=1", f.name],
            capture_output=True, text=True, check=True).stdout
    return dict(line.split("=") for line in out.split())


def ffmpeg_decode(data: bytes, suffix: str) -> bytes:
    with tempfile.NamedTemporaryFile(suffix=suffix) as f:
        f.write(data)
        f.flush()
        return subprocess.run(["ffmpeg", "-v", "error", "-i", f.name, "-f", "s16le", "-"],
                              capture_output=True, check=True).stdout


class SignalTests(unittest.TestCase):
    def test_deterministic_and_sized(self):
        a = fp.Signal((fp.Track(0.1), fp.Track(0.05, 880.0)), rate=8000, channels=2)
        self.assertEqual(a.pcm(), fp.Signal((fp.Track(0.1), fp.Track(0.05, 880.0)), rate=8000, channels=2).pcm())
        self.assertEqual(len(a.pcm()), (800 + 400) * 2 * 2)
        self.assertEqual(a.track_bounds(), [(0, 800), (800, 1200)])

    def test_endless_repeats(self):
        sig = fp.Signal((fp.Track(0.05),), rate=8000, channels=1, endless=True)
        it = sig.chunks(chunk_frames=400)
        first = [next(it) for _ in range(3)]  # 400 frames per track: one full pass, then again
        self.assertEqual(first[0], first[1])  # the second pass repeats the first
        with self.assertRaises(ValueError):
            sig.pcm()

    def test_wav_header_unknown_length(self):
        h = fp.wav_header(48000, 2)
        self.assertEqual(h[:4], b"RIFF")
        self.assertEqual(h[4:8], b"\xff\xff\xff\xff")
        self.assertEqual(h[-4:], b"\xff\xff\xff\xff")
        self.assertEqual(len(h), 44)


class NegotiationTests(unittest.TestCase):
    def test_ha_takes_first_default_or_first_announcement(self):
        self.assertEqual(fp.ha_pick_format(SAT_FORMATS).sample_rate, 48000)
        self.assertEqual(fp.ha_pick_format(SAT_FORMATS, announce=True).sample_rate, 22050)
        self.assertIsNone(fp.ha_pick_format(SAT_FORMATS[:1]))  # announcement only (today's satellite)

    def test_ma_codec_from_first_sorted_rate_from_all(self):
        # DEFAULT sorts first: codec/channels from 48 kHz stereo; the rate from the set {22050, 48000}.
        self.assertEqual(fp.ma_pick_format(SAT_FORMATS, 44100), ("flac", 48000, 2, 2))
        self.assertEqual(fp.ma_pick_format(SAT_FORMATS, 22050), ("flac", 22050, 2, 2))
        self.assertEqual(fp.ma_pick_format(SAT_FORMATS, 96000), ("flac", 48000, 2, 2))
        wav_first = [Fmt("wav", 48000, 2, 0), Fmt("flac", 44100, 2, 0)]
        self.assertEqual(fp.ma_pick_format(wav_first, 44100)[:2], ("wav", 44100))


class HaProxyTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.proxy = await fp.HaProxy(backend="python", max_conversions=2).start()
        self.addAsyncCleanup(self.proxy.stop)

    def url(self, device="dev", sig=SHORT, fmt="wav", rate=48000, ch=2):
        return self.proxy.create_url(device, fmt, rate, ch, signal=sig)

    async def test_plain_chunked_2048_no_length_no_auth(self):
        r = await fp.fetch(self.url())
        self.assertEqual(r.status, 200)
        self.assertEqual(r.headers.get("transfer-encoding"), "chunked")
        self.assertNotIn("content-length", r.headers)
        self.assertTrue(r.complete)
        self.assertTrue(r.chunks)
        self.assertTrue(all(c == 2048 for c in r.chunks[:-1]), set(r.chunks[:-1]))
        self.assertLessEqual(r.chunks[-1], 2048)
        self.assertEqual(r.body[:4], b"RIFF")
        self.assertEqual(r.body[44:], SHORT.with_format(48000, 2).pcm())  # the generated signal, bit exact

    async def test_range_and_auth_headers_ignored(self):
        u = self.url()
        plain = await fp.fetch(u)
        r = await fp.fetch(u, headers={"Range": "bytes=1000-", "Authorization": "Bearer nope"})
        self.assertEqual(r.status, 200)
        self.assertNotIn("content-range", r.headers)
        self.assertEqual(r.body, plain.body)  # the whole body from the start, not a range
        self.assertEqual(self.proxy.count("fetch", range="bytes=1000-"), 1)  # the Range header was seen and ignored

    async def test_a_new_request_restarts_the_conversion_from_the_start(self):
        u = self.url(sig=LONG)
        conv = self.proxy.conversion_of(u)
        first = await fp.fetch(u, max_bytes=20000)  # client goes away mid-way
        self.assertGreaterEqual(len(first.body), 20000)
        self.assertTrue(await until(lambda: conv.finished))
        second = await fp.fetch(u, max_bytes=20000)
        self.assertEqual(second.status, 200)
        self.assertEqual(first.body[:20000], second.body[:20000])  # starts at the beginning again
        self.assertEqual(conv.fetches, 2)

    async def test_a_request_while_streaming_cuts_the_old_one_and_restarts(self):
        u = self.url(sig=LONG)
        r1, w1, _head = await open_stream(u)
        await r1.read(4096)  # the first client is mid-body (and stops reading)
        r2, w2, head2 = await open_stream(u)
        self.assertTrue(head2.startswith(b"HTTP/1.1 200"))
        self.assertEqual(self.proxy.conversion_of(u).restarts, 1)

        async def drain_old():
            while await r1.read(65536):
                pass

        await asyncio.wait_for(drain_old(), 5)  # the old connection ends (cut by the restart)
        self.assertEqual((await r2.read(2))[:1], b"8")  # the new one streams (chunk size line "800")
        for w in (w1, w2):
            w.transport.abort()

    async def test_unfetched_urls_hold_slots_and_a_third_kills_the_oldest(self):
        a, b = self.url(), self.url()
        self.assertEqual(self.proxy.slot_count("dev"), 2)  # never fetched, still holding slots
        c = self.url()
        self.assertEqual(self.proxy.slot_count("dev"), 2)
        self.assertTrue(self.proxy.conversion_of(a).killed)
        self.assertEqual((await fp.fetch(a)).status, 404)  # the killed URL is gone
        self.assertEqual((await fp.fetch(b)).status, 200)
        self.assertEqual((await fp.fetch(c)).status, 200)

    async def test_third_url_cuts_the_oldest_running_conversion(self):
        a = self.url(sig=LONG)
        r, w, head = await open_stream(a)
        self.assertTrue(head.startswith(b"HTTP/1.1 200"))
        self.url(sig=LONG)
        self.url(sig=LONG)  # third unfinished: the oldest (running) one dies
        self.assertTrue(self.proxy.conversion_of(a).killed)

        async def drain():
            while await r.read(65536):
                pass

        await asyncio.wait_for(drain(), 5)  # connection closed without a clean end
        w.transport.abort()
        self.assertEqual(self.proxy.slot_count("dev"), 2)

    async def test_finished_and_dropped_conversions_free_their_slot(self):
        a, b = self.url(), self.url()
        await fp.fetch(a)  # short: fully served, finished
        self.assertTrue(await until(lambda: self.proxy.slot_count("dev") == 1))
        c = self.url(sig=LONG)
        await fp.fetch(c, max_bytes=100)  # dropped by the client: opened and closed
        self.assertTrue(await until(lambda: self.proxy.slot_count("dev") == 1))  # only b still holds one
        self.url()
        self.assertFalse(self.proxy.conversion_of(b).killed)  # nothing was evicted: the slots were free
        self.assertEqual(self.proxy.slot_count("dev"), 2)

    async def test_urls_are_not_one_time(self):
        u = self.url()
        self.assertEqual((await fp.fetch(u)).body, (await fp.fetch(u)).body)
        self.assertEqual(self.proxy.conversion_of(u).fetches, 2)

    async def test_slots_are_per_device(self):
        for d in ("d1", "d2", "d3"):
            self.url(d)
            self.url(d)
        self.assertTrue(all(self.proxy.slot_count(d) == 2 for d in ("d1", "d2", "d3")))

    async def test_slow_client_blocks_the_conversion(self):
        u = self.url(sig=LONG)
        r, w, _head = await open_stream(u)
        await asyncio.sleep(0.5)  # the client does not read: backpressure
        sent = self.proxy.conversion_of(u).active is not None
        self.assertTrue(sent)
        self.assertFalse(self.proxy.conversion_of(u).finished)
        w.transport.abort()
        self.assertTrue(await until(lambda: self.proxy.conversion_of(u).finished))

    async def test_unknown_and_wrong_requests(self):
        self.assertEqual((await fp.fetch(self.proxy.base_url + "/api/esphome/ffmpeg_proxy/dev/nope.wav")).status, 404)
        self.assertEqual((await fp.fetch(self.proxy.base_url + "/other")).status, 404)
        u = self.url("dev")
        self.assertEqual((await fp.fetch(u.replace("/dev/", "/other-device/"))).status, 404)

    async def test_play_media_helper_uses_the_negotiated_format(self):
        u = self.proxy.url_for_play_media("dev", [Fmt("wav", 22050, 1, 1), Fmt("wav", 48000, 2, 0)], announce=False)
        self.assertEqual(self.proxy.conversion_of(u).rate, 48000)
        u = self.proxy.url_for_play_media("dev", [Fmt("wav", 22050, 1, 1), Fmt("wav", 48000, 2, 0)], announce=True)
        self.assertEqual((self.proxy.conversion_of(u).rate, self.proxy.conversion_of(u).channels), (22050, 1))
        self.assertTrue(u.endswith(".wav"))

    async def test_http10_client_gets_a_close_delimited_body(self):
        r = await fp.fetch(self.url(), version="1.0")
        self.assertNotIn("transfer-encoding", r.headers)
        self.assertTrue(r.complete)
        self.assertEqual(r.body[44:], SHORT.with_format(48000, 2).pcm())

    async def test_faults(self):
        self.proxy.fault = fp.Fault(status=500)
        self.assertEqual((await fp.fetch(self.url())).status, 500)
        self.proxy.fault = fp.Fault(close_after=5000)
        r = await fp.fetch(self.url(sig=LONG))
        self.assertEqual(len(r.body), 5000)
        self.assertTrue(r.truncated)
        self.proxy.fault = fp.Fault(stall_after=4096)
        r2, w2, _h = await open_stream(self.url(sig=LONG))
        got = b""
        with self.assertRaises(TimeoutError):  # sends 4096 bytes of body, then goes silent
            while True:
                got += await asyncio.wait_for(r2.read(65536), 0.5)
        self.assertGreaterEqual(len(got), 4096)
        w2.transport.abort()


@unittest.skipUnless(SLOW, SLOW_REASON)
@unittest.skipUnless(HAVE_FFMPEG, "ffmpeg/ffprobe not installed")
class HaProxyFfmpegTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.proxy = await fp.HaProxy(backend="ffmpeg").start()
        self.addAsyncCleanup(self.proxy.stop)

    async def test_output_has_the_url_format_rate_and_channels(self):
        for fmt, rate, ch in (("flac", 48000, 2), ("flac", 22050, 1), ("wav", 44100, 2), ("wav", 16000, 1)):
            with self.subTest(fmt=fmt, rate=rate, ch=ch):
                u = self.proxy.create_url("dev", fmt, rate, ch, signal=SHORT)  # the source is 44.1 kHz stereo
                r = await fp.fetch(u)
                self.assertEqual(r.status, 200)
                self.assertTrue(all(c == 2048 for c in r.chunks[:-1]))
                self.assertNotIn("content-length", r.headers)
                self.assertEqual(r.headers["content-type"], fp.MIME[fmt])
                info = ffprobe(r.body, "." + fmt)
                self.assertEqual((int(info["sample_rate"]), int(info["channels"])), (rate, ch))
                self.assertTrue(info["codec_name"].startswith("pcm_s16le" if fmt == "wav" else "flac"))

    async def test_flac_decodes_to_the_source(self):
        u = self.proxy.create_url("dev", "flac", 44100, 2, signal=SHORT)  # same format as the source: lossless
        r = await fp.fetch(u)
        self.assertEqual(ffmpeg_decode(r.body, ".flac"), SHORT.pcm())

    async def test_restart_and_eviction_with_real_ffmpeg(self):
        a = self.proxy.create_url("dev", "flac", 48000, 2, signal=LONG)
        # LONG is generated in Python before ffmpeg starts: on a loaded
        # machine that took longer than fetch's default 15 s (not what this
        # test is about).
        first = await fp.fetch(a, max_bytes=30000, timeout=60)
        self.assertTrue(await until(lambda: self.proxy.conversion_of(a).finished, timeout=60))
        second = await fp.fetch(a, max_bytes=30000, timeout=60)
        self.assertEqual(first.body[:30000], second.body[:30000])
        # a blocked (slow client) conversion is killed by the third URL
        r, w, _head = await open_stream(a)
        self.proxy.create_url("dev", "flac", 48000, 2, signal=LONG)
        self.proxy.create_url("dev", "flac", 48000, 2, signal=LONG)
        self.assertTrue(self.proxy.conversion_of(a).killed)

        async def drain():
            while await r.read(65536):
                pass

        await asyncio.wait_for(drain(), 5)
        w.transport.abort()


class MaFlowTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.ma = await fp.MaFlow(backend="python").start()
        self.addAsyncCleanup(self.ma.stop)

    async def test_finite_multi_track_stream_without_length(self):
        tracks = (fp.Track(0.2, 330.0), fp.Track(0.2, 440.0), fp.Track(0.2, 550.0))
        for framing in ("chunked", "close"):
            with self.subTest(framing=framing):
                u = self.ma.create_stream(fmt="wav", rate=44100, channels=2, tracks=tracks, framing=framing)
                r = await fp.fetch(u)
                self.assertEqual(r.status, 200)
                self.assertNotIn("content-length", r.headers)
                self.assertEqual(r.headers.get("transfer-encoding"), "chunked" if framing == "chunked" else None)
                self.assertTrue(r.complete)
                sig = self.ma.stream_of(u).signal
                self.assertEqual(r.body[44:], sig.pcm())  # all three tracks back to back, one stream
                self.assertEqual(len(sig.track_bounds()), 3)

    async def test_other_host_and_port_than_the_ha_proxy(self):
        async with fp.HaProxy(backend="python") as ha:
            self.assertNotEqual(ha.port, self.ma.port)
            self.assertNotIn("/api/esphome/ffmpeg_proxy/", self.ma.create_stream(fmt="wav"))
        async with fp.MaFlow(host="127.0.0.2", backend="python") as other:  # a different host (loopback alias)
            self.assertEqual(other.host, "127.0.0.2")
            self.assertEqual((await fp.fetch(other.create_stream(fmt="wav"), max_bytes=100)).status, 200)

    async def test_endless_stream_never_ends_and_the_server_notices_the_close(self):
        u = self.ma.create_stream(fmt="wav", rate=22050, channels=1, endless=True)
        s = self.ma.stream_of(u)
        r = await fp.fetch(u, max_bytes=300000)  # far more than one pass of the three 1 s tracks
        self.assertGreaterEqual(len(r.body), 300000)
        self.assertFalse(r.complete)
        self.assertFalse(r.truncated)
        self.assertTrue(await until(lambda: s.open == 0))

    async def test_end_abort_closes_without_a_terminal_chunk(self):
        u = self.ma.create_stream(fmt="wav", tracks=[fp.Track(0.05)], end="abort")
        r = await fp.fetch(u)
        self.assertTrue(r.truncated)
        self.assertFalse(r.complete)
        self.assertEqual(r.body[44:], self.ma.stream_of(u).signal.pcm())

    async def test_not_one_time_not_slot_limited(self):
        u = self.ma.create_stream(fmt="wav", endless=True)
        conns = [await open_stream(u) for _ in range(5)]
        s = self.ma.stream_of(u)
        self.assertEqual((s.fetches, s.open, s.max_open), (5, 5, 5))  # five at once, none evicted
        for r, w, head in conns:
            self.assertTrue(head.startswith(b"HTTP/1.1 200"))
            self.assertNotIn(b"Content-Length", head)
            w.transport.abort()
        self.assertTrue(await until(lambda: s.open == 0))
        self.assertEqual((await fp.fetch(u, max_bytes=100)).status, 200)  # and fetchable again later

    async def test_format_follows_the_ma_rule(self):
        u = self.ma.url_for_play_media(SAT_FORMATS, source_rate=44100)
        s = self.ma.stream_of(u)
        self.assertEqual((s.fmt, s.rate, s.channels), ("flac", 48000, 2))

    async def test_unknown_stream_and_faults(self):
        self.assertEqual((await fp.fetch(self.ma.base_url + "/flow/x/q.wav")).status, 404)
        u = self.ma.create_stream(fmt="wav", endless=True, fault=fp.Fault(status=404))
        self.assertEqual((await fp.fetch(u)).status, 404)
        u = self.ma.create_stream(fmt="wav", endless=True, fault=fp.Fault(close_after=1000))
        r = await fp.fetch(u)
        self.assertEqual(len(r.body), 1000)
        self.assertTrue(r.truncated)


@unittest.skipUnless(SLOW, SLOW_REASON)
@unittest.skipUnless(HAVE_FFMPEG, "ffmpeg/ffprobe not installed")
class MaFlowFfmpegTests(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.ma = await fp.MaFlow(backend="ffmpeg").start()
        self.addAsyncCleanup(self.ma.stop)

    async def test_44100_stereo_flac_stream_decodes_to_all_tracks(self):
        u = self.ma.create_stream(fmt="flac", rate=44100, channels=2,
                                  tracks=[fp.Track(0.3, 330.0), fp.Track(0.3, 440.0)])
        r = await fp.fetch(u)
        self.assertNotIn("content-length", r.headers)
        self.assertTrue(r.complete)
        info = ffprobe(r.body, ".flac")
        self.assertEqual((info["codec_name"], int(info["sample_rate"]), int(info["channels"])), ("flac", 44100, 2))
        self.assertEqual(ffmpeg_decode(r.body, ".flac"), self.ma.stream_of(u).signal.pcm())

    async def test_endless_flac_and_other_rates(self):
        for rate, ch in ((44100, 2), (22050, 1), (48000, 2)):
            with self.subTest(rate=rate, ch=ch):
                u = self.ma.create_stream(fmt="flac", rate=rate, channels=ch, endless=True)
                r = await fp.fetch(u, max_bytes=200000)
                self.assertGreaterEqual(len(r.body), 200000)
                self.assertFalse(r.complete)
                info = ffprobe(r.body[:100000], ".flac")
                self.assertEqual((int(info["sample_rate"]), int(info["channels"])), (rate, ch))
                self.assertTrue(await until(lambda: self.ma.stream_of(u).open == 0))


@unittest.skipUnless(HAVE_OPENSSL, "openssl not installed")
class TlsTests(unittest.IsolatedAsyncioTestCase):
    async def test_https_with_a_private_ca(self):
        with tempfile.TemporaryDirectory() as d:
            ca, cert, key = fp.make_private_ca(d)
            async with fp.HaProxy(backend="python", tls=(cert, key)) as proxy:
                u = proxy.create_url("dev", "wav", 48000, 2, signal=SHORT)
                self.assertTrue(u.startswith("https://"))
                r = await fp.fetch(u, ca_file=ca)
                self.assertEqual((r.status, r.complete), (200, True))
                with self.assertRaises(Exception):  # the system roots do not trust it
                    await fp.fetch(u)


if __name__ == "__main__":
    unittest.main()
