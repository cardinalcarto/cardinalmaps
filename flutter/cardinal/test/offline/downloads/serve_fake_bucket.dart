// Serves a `cardinal-publish --target dir:` bucket as the release bucket does (ranges,
// ETag and If-Match, Content-Encoding), for running the phone's end-to-end test against
// it through `adb reverse`:
//
//   dart run test/offline/downloads/serve_fake_bucket.dart DIR PORT [BYTES_PER_SECOND]
import 'dart:io';

import 'fake_bucket.dart';

Future<void> main(List<String> args) async {
  if (args.length < 2) {
    stderr.writeln('usage: serve_fake_bucket.dart DIR PORT [BYTES_PER_SECOND]');
    exit(64);
  }
  final bucket = await FakeBucket.serve(args[0], port: int.parse(args[1]));
  if (args.length > 2) bucket.bytesPerSecond = int.parse(args[2]);
  stdout.writeln('serving ${args[0]} at ${bucket.releasesUrl}');
  await ProcessSignal.sigterm.watch().first;
  await bucket.close();
}
