import hashlib
import heapq
import importlib
import time
import unittest
from threading import BoundedSemaphore, Event
from unittest.mock import Mock, patch

from botocore.awsrequest import AWSResponse
from botocore.config import Config
from google.transit import gtfs_realtime_pb2

import updater


class UpdaterTests(unittest.TestCase):
    def test_connection_pool_matches_worker_count(self):
        with patch.object(updater, 'WORKERS', 23), \
                patch.object(updater.boto3, 'client') as client, \
                patch.object(updater, 'run'):
            updater.main()
        self.assertEqual(client.call_args.kwargs['config'].max_pool_connections, 23)

    def test_default_release_url_uses_public_hostname(self):
        with patch.dict('os.environ', {}, clear=True):
            importlib.reload(updater)
            self.assertEqual(updater.RELEASES,
                             'https://cardinal-carto-public.t3.tigrisfiles.io/releases')
        importlib.reload(updater)

    def test_key_matches_client(self):
        self.assertEqual(updater.object_key('https://example.org/trip.pb'),
                         'gtfs-rt/a5dffcef4b30cc6eb129bdceefea1a4930bfd8371943d4143eb54779f3d9a6ad.pb')

    def test_sources_follow_latest_release_and_deduplicate(self):
        latest = Mock()
        latest.json.return_value = {'transit': '2026-11-26'}
        manifest = Mock()
        manifest.json.return_value = {'feeds': [
            {'id': 'one', 'url': 'https://example.org/trip.pb'},
            {'id': 'two', 'url': 'https://example.org/trip.pb'},
            {'id': 'two', 'url': 'https://example.org/vehicle.pb'},
        ]}
        with patch.object(updater.requests, 'get', side_effect=[latest, manifest]) as get:
            self.assertEqual(updater.sources(), {
                'https://example.org/trip.pb', 'https://example.org/vehicle.pb',
            })
            self.assertEqual(get.call_args_list[1].args[0],
                             f'{updater.RELEASES}/transit/2026-11-26/realtime.json')

    def test_valid_feed_published_unmodified_with_freshness_metadata(self):
        message = gtfs_realtime_pb2.FeedMessage()
        message.header.gtfs_realtime_version = '2.0'
        body = message.SerializeToString()
        s3 = Mock()
        uploads = []
        def put_object(**kwargs):
            stream = kwargs.pop('Body')
            self.assertTrue(stream.seekable())
            self.assertEqual(stream.tell(), 0)
            uploads.append((kwargs, stream.read()))
        s3.put_object.side_effect = put_object
        response = self.response(body)
        with patch.object(updater.requests, 'get', return_value=response), \
                patch.object(updater.time, 'time', return_value=1000):
            result = updater.refresh(s3, 'https://example.org/trip.pb', BoundedSemaphore(1), 0)
        self.assertTrue(result.succeeded)
        self.assertEqual(uploads, [(dict(
            Bucket=updater.BUCKET, Key=updater.object_key('https://example.org/trip.pb'),
            ContentLength=len(body), ContentType='application/x-protobuf', CacheControl='no-cache',
            Metadata={'fetched-at': '1000'},
        ), body)])

    def test_failures_do_not_replace_previous_object(self):
        for body in (b'', b'<html>error</html>', b'\xff'):
            s3 = Mock()
            with patch.object(updater.requests, 'get', return_value=self.response(body)):
                result = updater.refresh(s3, 'https://example.org/trip.pb', BoundedSemaphore(1), 0)
            s3.put_object.assert_not_called()
            self.assertEqual(result.failed_stage, 'validation')
        s3 = Mock()
        with patch.object(updater.requests, 'get', side_effect=TimeoutError):
            result = updater.refresh(s3, 'https://example.org/trip.pb', BoundedSemaphore(1), 0)
        s3.put_object.assert_not_called()
        self.assertEqual(result.failed_stage, 'download')

    def test_upload_failure_is_distinguished_from_download_failure(self):
        message = gtfs_realtime_pb2.FeedMessage()
        message.header.gtfs_realtime_version = '2.0'
        s3 = Mock()
        s3.put_object.side_effect = TimeoutError
        with patch.object(updater.requests, 'get', return_value=self.response(message.SerializeToString())):
            result = updater.refresh(s3, 'https://example.org/trip.pb', BoundedSemaphore(1), 0)
        self.assertEqual(result.failed_stage, 'upload')
        self.assertFalse(result.succeeded)

    def test_oversized_feed_is_not_uploaded(self):
        s3 = Mock()
        with patch.object(updater, 'MAX_BYTES', 2), \
                patch.object(updater.requests, 'get', return_value=self.response(b'123')):
            result = updater.refresh(s3, 'https://example.org/trip.pb', BoundedSemaphore(1), 0)
        self.assertEqual(result.failed_stage, 'download')
        s3.put_object.assert_not_called()

    def test_boto_streams_file_and_can_rewind_on_retry(self):
        message = gtfs_realtime_pb2.FeedMessage()
        message.header.gtfs_realtime_version = '2.0'
        message.entity.add(id='x' * (256 * 1024))
        body = message.SerializeToString()
        response = self.response(body)
        response.__enter__.return_value.iter_content.return_value = [
            body[i:i + 65536] for i in range(0, len(body), 65536)
        ]
        s3 = updater.boto3.client('s3', endpoint_url='https://example.invalid', region_name='auto',
                                  aws_access_key_id='test', aws_secret_access_key='test',
                                  config=Config(retries={'total_max_attempts': 2}))
        attempts = []

        def send(request):
            self.assertTrue(hasattr(request.body, 'read'))
            digest = hashlib.sha256()
            while chunk := request.body.read(65536):
                digest.update(chunk)
            attempts.append(digest.digest())
            raw = Mock()
            raw.stream.return_value = iter([b''])
            return AWSResponse(request.url, 500 if len(attempts) == 1 else 200, {}, raw)

        with patch.object(updater.requests, 'get', return_value=response), \
                patch.object(s3._endpoint.http_session, 'send', side_effect=send), \
                patch('botocore.endpoint.time.sleep'):
            result = updater.refresh(s3, 'https://example.org/trip.pb', BoundedSemaphore(1), 0)
        self.assertTrue(result.succeeded)
        self.assertEqual(len(attempts), 2)
        self.assertEqual(attempts[0], attempts[1])

    @staticmethod
    def response(body):
        response = Mock(status_code=200)
        response.iter_content.return_value = [body]
        context = Mock()
        context.__enter__ = Mock(return_value=response)
        context.__exit__ = Mock(return_value=False)
        return context


class SchedulerTests(unittest.TestCase):
    def test_executor_releases_fast_feed_before_slow_feed_finishes(self):
        slow_started = Event()
        release_slow = Event()
        fast_refreshed_while_slow = Event()
        fast_calls = 0

        class Finished(Exception):
            pass

        class StoppingScheduler(updater.Scheduler):
            def complete(self, url, result, now):
                super().complete(url, result, now)
                if url == 'slow':
                    raise Finished

        def refresh(s3, url, validators, due_at):
            nonlocal fast_calls
            if url == 'slow':
                slow_started.set()
                release_slow.wait(2)
            else:
                fast_calls += 1
                if fast_calls == 2:
                    if slow_started.is_set():
                        fast_refreshed_while_slow.set()
                    release_slow.set()
            return updater.RefreshResult(fetched_at=int(time.time()))

        with patch.object(updater, 'INTERVAL', 0.05), \
                patch.object(updater, 'WORKERS', 2), \
                patch.object(updater, 'Scheduler', StoppingScheduler), \
                patch.object(updater, 'sources', return_value={'fast', 'slow'}), \
                patch.object(updater, 'refresh', side_effect=refresh):
            with self.assertRaises(Finished):
                updater.run(Mock())
        self.assertTrue(fast_refreshed_while_slow.is_set())

    def test_fast_feed_refreshes_while_unrelated_feed_is_still_running(self):
        scheduler = updater.Scheduler()
        scheduler.reconcile({'fast', 'slow'}, 0)
        self.assertEqual(scheduler.ready(0, 2), [(0, 'fast'), (0, 'slow')])
        scheduler.complete('fast', updater.RefreshResult(fetched_at=2), 2)
        self.assertEqual(scheduler.ready(59, 1), [])
        self.assertEqual(scheduler.ready(60, 1), [(60, 'fast')])
        self.assertEqual(scheduler.running, {'fast', 'slow'})
        # No overlapping refresh of either URL.
        self.assertEqual(scheduler.ready(120, 2), [])

    def test_backoff_is_independent_and_resets_on_success(self):
        scheduler = updater.Scheduler()
        scheduler.reconcile({'fast', 'slow'}, 0)
        scheduler.ready(0, 2)
        scheduler.complete('fast', updater.RefreshResult(fetched_at=1), 1)
        scheduler.complete('slow', updater.RefreshResult(failed_stage='download'), 2)
        self.assertEqual(scheduler.next_due(), 17)
        scheduler.ready(17, 1)
        scheduler.complete('slow', updater.RefreshResult(failed_stage='upload'), 18)
        self.assertEqual(scheduler.next_due(), 48)
        scheduler.ready(48, 1)
        scheduler.complete('slow', updater.RefreshResult(fetched_at=49), 49)
        self.assertEqual(scheduler.feeds['slow'].failures, 0)
        self.assertEqual(scheduler.next_due(), 60)
        self.assertEqual(scheduler.ready(60, 2), [(60, 'fast')])

    def test_removing_and_readding_inflight_source_does_not_duplicate_it(self):
        scheduler = updater.Scheduler()
        scheduler.reconcile({'a', 'b'}, 0)
        scheduler.ready(0, 1)
        scheduler.reconcile({'b'}, 1)
        scheduler.reconcile({'a', 'b'}, 2)
        self.assertEqual(scheduler.ready(2, 2), [(0, 'b')])
        scheduler.reconcile({'a'}, 3)
        scheduler.complete('b', updater.RefreshResult(fetched_at=3), 3)
        self.assertNotIn('b', scheduler.feeds)
        scheduler.complete('a', updater.RefreshResult(fetched_at=4), 4)
        self.assertEqual(scheduler.next_due(), 60)

    def test_overdue_feeds_get_priority_and_do_not_catch_up_in_bursts(self):
        scheduler = updater.Scheduler()
        scheduler.reconcile({'a', 'b'}, 0)
        scheduler.ready(0, 1)
        scheduler.complete('a', updater.RefreshResult(fetched_at=100), 100)
        self.assertEqual(scheduler.ready(100, 1), [(0, 'b')])
        scheduler.complete('b', updater.RefreshResult(fetched_at=101), 101)
        self.assertEqual(scheduler.ready(101, 1), [(100, 'a')])
        scheduler.complete('a', updater.RefreshResult(fetched_at=102), 102)
        self.assertEqual(scheduler.next_due(), 160)

    def test_1051_sources_with_slow_and_failing_feeds(self):
        scheduler = updater.Scheduler()
        urls = {f'{i:04d}' for i in range(1051)}
        scheduler.reconcile(urls, 0)
        events = []
        last = {}
        intervals = []
        now = 0
        while now < 600:
            while events and events[0][0] <= now:
                done, url, successful = heapq.heappop(events)
                result = (updater.RefreshResult(fetched_at=int(done)) if successful else
                          updater.RefreshResult(failed_stage='download'))
                scheduler.complete(url, result, now)
                if successful and int(url) >= 50:
                    if url in last:
                        intervals.append(done - last[url])
                    last[url] = done
            for _, url in scheduler.ready(now, 64 - len(events)):
                slow = int(url) < 50
                heapq.heappush(events, (now + (30 if slow else 0.5), url,
                                        not slow or int(url) % 2 == 0))
            next_done = events[0][0] if events else float('inf')
            next_due = scheduler.next_due() if len(events) < 64 else float('inf')
            now = min(next_done, next_due)
        self.assertEqual(len(last), 1001)
        self.assertGreater(len(intervals), 7000)
        self.assertLessEqual(max(intervals), 90)


if __name__ == '__main__':
    unittest.main()
