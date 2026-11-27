import unittest
from unittest.mock import Mock, patch

from google.transit import gtfs_realtime_pb2

import diagnose
import test_updater


class DiagnoseTests(unittest.TestCase):
    def test_probe_reports_cache_and_agency_ages_without_exposing_url(self):
        message = gtfs_realtime_pb2.FeedMessage()
        message.header.gtfs_realtime_version = '2.0'
        message.header.timestamp = 900
        message.entity.add(id='a').trip_update.trip.trip_id = 'trip'
        body = message.SerializeToString()
        response = test_updater.UpdaterTests.response(body)
        response.__enter__.return_value.headers = {'x-amz-meta-fetched-at': '950'}
        upstream = 'https://example.org/token-secret/trip.pb?api_key=secret'
        with patch.object(diagnose.requests, 'get', return_value=response), \
                patch.object(diagnose.time, 'time', return_value=1000):
            result = diagnose.probe(upstream, 'https://cache.example.org/feed.pb', 'cache')
        self.assertEqual(result['header_age_s'], 100)
        self.assertEqual(result['cache_age_s'], 50)
        self.assertEqual(result['trip_updates'], 1)
        self.assertNotIn('secret', str(result))

    def test_pinned_release_selects_and_deduplicates_only_requested_feed(self):
        response = Mock()
        response.json.return_value = {'feeds': [
            {'id': 'metro', 'url': 'https://example.org/a'},
            {'id': 'metro', 'url': 'https://example.org/a'},
            {'id': 'other', 'url': 'https://example.org/b'},
        ]}
        with patch.object(diagnose.requests, 'get', return_value=response) as get:
            release, urls = diagnose.manifest_sources('metro', '2026-11-12')
        self.assertEqual(release, '2026-11-12')
        self.assertEqual(urls, ['https://example.org/a'])
        get.assert_called_once_with(f'{diagnose.RELEASES}/transit/2026-11-12/realtime.json', timeout=15)
