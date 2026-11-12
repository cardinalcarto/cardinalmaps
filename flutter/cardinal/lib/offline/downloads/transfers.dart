import 'dart:async';
import 'dart:io';

import 'package:background_downloader/background_downloader.dart';
import 'package:flutter/foundation.dart';

/// A file to download in the background, to [path], with [headers] (a byte range and
/// `If-Match` for map ranges).
@immutable
class TransferJob {
  const TransferJob({
    required this.id,
    required this.url,
    required this.path,
    this.headers = const {},
  });

  final String id;
  final Uri url;

  /// Absolute. The file appears there only once it's complete.
  final String path;
  final Map<String, String> headers;
}

enum TransferOutcome { complete, failed, notFound, canceled }

/// How a [TransferJob] ended.
@immutable
class TransferResult {
  const TransferResult(
    this.id,
    this.outcome, {
    this.statusCode,
    this.headers = const {},
    this.error,
  });

  final String id;
  final TransferOutcome outcome;

  /// The response's status, when there was one.
  final int? statusCode;

  /// The response's headers, lowercased; empty when the platform didn't report them
  /// (a download that finished while the app wasn't running).
  final Map<String, String> headers;
  final String? error;

  @override
  String toString() =>
      'TransferResult($id, ${outcome.name}, $statusCode${error == null ? '' : ', $error'})';
}

/// What downloads run on, behind what the engine needs of it, so tests can fake it.
abstract class Transfers {
  /// How jobs end, including ones that ended while the app wasn't running (reported
  /// after [start]).
  Stream<TransferResult> get results;

  /// Starts reporting; call once, after listening to [results].
  Future<void> start();

  Future<void> enqueue(List<TransferJob> jobs);

  /// The IDs of jobs that haven't ended.
  Future<Set<String>> active();

  Future<void> cancel(List<String> ids);
}

/// [Transfers] on the `background_downloader` package: Android's WorkManager, iOS's
/// background URL session, and an isolate on desktops, so downloads go on while the app
/// is in the background.
class BackgroundTransfers implements Transfers {
  BackgroundTransfers({FileDownloader? downloader})
    : _downloader = downloader ?? FileDownloader();

  /// The downloader group offline downloads use.
  static const group = 'cardinal-offline';

  final FileDownloader _downloader;
  final _results = StreamController<TransferResult>.broadcast();
  StreamSubscription<TaskUpdate>? _updates;

  @override
  Stream<TransferResult> get results => _results.stream;

  @override
  Future<void> start() async {
    _updates ??= _downloader.updates.listen(_update);
    // Records tasks, so downloads that finished while the app was away (their files in
    // place) are reported as complete, and reschedules those the system killed.
    await _downloader.start();
  }

  void _update(TaskUpdate update) {
    if (update is! TaskStatusUpdate || update.task.group != group) return;
    final outcome = switch (update.status) {
      TaskStatus.complete => TransferOutcome.complete,
      TaskStatus.failed => TransferOutcome.failed,
      TaskStatus.notFound => TransferOutcome.notFound,
      TaskStatus.canceled => TransferOutcome.canceled,
      _ => null,
    };
    if (outcome == null) return;
    final exception = update.exception;
    _results.add(
      TransferResult(
        update.task.taskId,
        outcome,
        statusCode:
            update.responseStatusCode ??
            (exception is TaskHttpException
                ? exception.httpResponseCode
                : null),
        headers: {
          for (final entry in (update.responseHeaders ?? const {}).entries)
            entry.key.toLowerCase(): entry.value,
        },
        error: exception?.description,
      ),
    );
  }

  @override
  Future<void> enqueue(List<TransferJob> jobs) async {
    final tasks = [
      for (final job in jobs)
        DownloadTask(
          taskId: job.id,
          url: job.url.toString(),
          filename: job.path.split(Platform.pathSeparator).last,
          directory: File(job.path).parent.path,
          baseDirectory: BaseDirectory.root,
          headers: job.headers,
          group: group,
          updates: Updates.status,
          // A broken connection resumes where it stopped rather than starting over;
          // failures are counted and retried by the engine.
          allowPause: true,
          retries: 0,
        ),
    ];
    final queued = await _downloader.enqueueAll(tasks);
    for (final (i, ok) in queued.indexed) {
      if (!ok) {
        _results.add(
          TransferResult(
            tasks[i].taskId,
            TransferOutcome.failed,
            error: 'not enqueued',
          ),
        );
      }
    }
  }

  @override
  Future<Set<String>> active() async =>
      (await _downloader.allTaskIds(group: group)).toSet();

  @override
  Future<void> cancel(List<String> ids) async {
    if (ids.isNotEmpty) await _downloader.cancelTasksWithIds(ids);
  }
}
