import importlib
import unittest
from unittest.mock import Mock, patch

from google.transit import gtfs_realtime_pb2

import updater


class UpdaterTests(unittest.TestCase):
    def test_connection_pool_matches_worker_count(self):
        with patch.object(updater, 'WORKERS', 23), \
                patch.object(updater.boto3, 'client') as client, \
                patch.object(updater, 'sources', return_value=set()), \
                patch.object(updater.time, 'sleep', side_effect=KeyboardInterrupt):
            with self.assertRaises(KeyboardInterrupt):
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
        response = self.response(body)
        with patch.object(updater.requests, 'get', return_value=response), \
                patch.object(updater.time, 'time', return_value=1000):
            updater.refresh(s3, 'https://example.org/trip.pb')
        s3.put_object.assert_called_once_with(
            Bucket=updater.BUCKET,
            Key=updater.object_key('https://example.org/trip.pb'),
            Body=body, ContentType='application/x-protobuf', CacheControl='no-cache',
            Metadata={'fetched-at': '1000'},
        )

    def test_failures_do_not_replace_previous_object(self):
        for body in (b'', b'<html>error</html>', b'\xff'):
            s3 = Mock()
            with patch.object(updater.requests, 'get', return_value=self.response(body)):
                updater.refresh(s3, 'https://example.org/trip.pb')
            s3.put_object.assert_not_called()
        s3 = Mock()
        with patch.object(updater.requests, 'get', side_effect=TimeoutError):
            updater.refresh(s3, 'https://example.org/trip.pb')
        s3.put_object.assert_not_called()

    @staticmethod
    def response(body):
        response = Mock(status_code=200)
        response.iter_content.return_value = [body]
        context = Mock()
        context.__enter__ = Mock(return_value=response)
        context.__exit__ = Mock(return_value=False)
        return context


if __name__ == '__main__':
    unittest.main()
