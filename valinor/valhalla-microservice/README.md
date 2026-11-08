# Valhalla microservice foundations

This crate provides a set of common data structures and traits
to hide all the implementation complexity of building a microservice for Valhalla.

## How Valhalla microservices work

If you've only ever run `valhalla_service`, you might not realize that Valhalla is actually a collection of microservices!
While most Valhalla users just use `valhalla_service` (which creates all the microservices in a single process),
you can also run each microservice individually.
The original reason for this was to enable independent service scalability,
but it also provides an extension point which allows for alternate implementations of specific services.
Pretty clever design!

All communication happens via ZeroMQ.
Messages are passed in Protobuf format using a standard schema (see the [valhalla-proto crate](../valhalla-proto)).
The classic `valhalla_service` is a single process running all the services,
but it doesn't have to be this way.
In fact, all the communication happens over ZeroMQ,
so you can run each service in a separate process,
or even on a different machine.

On top of ZeroMQ as the base layer,
Valhalla uses [`prime_server`](https://github.com/kevinkreiser/prime_server)
to define "workers" (the service's processing loop),
and handle the work distribution.

The services are effectively chained together via these sockets,
passing along multi-part ZMQ messages with a header and a protobuf payload.
These get updated as the message is processed,
and the final response is sent back to the client.

## The worker protocol

This crate implements `prime_server`'s worker (`worker_t` in `prime_server.cpp`)
as Valhalla 3.9.1 uses it.
Every message about a request starts with the server's 12-byte request info
([`http_protocol::HttpRequestInfo`]).

- **Upstream** (DEALER, connected to `<service>_out`):
  the worker advertises with an empty frame,
  after every job and again every second while idle,
  and receives jobs as `[request info, Api]`.
- **Downstream** (DEALER, connected to `<next service>_in`):
  [`WorkerResult::Forward`] sends `[request info, Api]`,
  with the request info exactly as received.
- **Loopback** (PUSH, connected to `httpd.service.loopback`):
  [`WorkerResult::HttpResponse`] sends `[request info, HTTP response]`,
  with the response code recorded in the request info.
  Errors go the same way, serialized by
  [`ValhallaError`](valhalla_response::ValhallaError) for the request's format
  (see [`WorkerResult::error`]),
  which is how mainline's services answer from whichever stage fails.
- **Interrupts** (SUB, connected to `httpd.service.interrupt`):
  the server publishes the 8-byte key (ID and timestamp) of each request it gives up on.
  The worker skips jobs that were already interrupted,
  and worker functions can check during long work ([`Interrupt::check`]).
  Interrupted jobs send nothing.

Workers receive `Api` messages, so this crate suits every stage after Loki
(Loki receives raw HTTP requests from the server).
[`Stage`] chooses which mainline service's internal errors to mirror
when the payload cannot be decoded or the worker function panics.

## Differences from `prime_server`

- The worker function runs on Tokio's blocking thread pool,
  and interrupts are read on a background task as they arrive
  rather than one at a time when the worker checks.
  The observable behavior is the same, except that checks never lag behind delivered interrupts.
- We do not support custom heartbeats (Valhalla's are always empty)
  or a cleanup callback (worker functions can clean up before returning).
- [zmq.rs](https://github.com/zeromq/zmq.rs) only reconnects SUB sockets,
  and a send with no connected peer fails at once, where libzmq would reconnect and queue.
  So when a send fails because its peer is gone,
  the service connects again and retries the send once,
  which is what lets a worker survive a proxy restart.
  This is tested for the upstream proxy.
  It does not help with the loopback in one case we tried:
  after dropping the PULL in-process and binding a new one at the same address,
  the PUSH kept sending successfully, and every response was lost.
  Whether the same happens when the server process restarts is untested,
  so restart workers when you restart the server.

## Crate design choices

- We do not wrap the underlying C++ libraries; everything is implemented in Rust.
- We're using the pure Rust implementation of ZeroMQ, even though it's experimental
  (this really just means it doesn't have feature parity, not that it is unsafe to use).
- We use [tracing](https://github.com/tokio-rs/tracing), and (for binaries) enable configuration with [`RUST_LOG`](https://docs.rs/env_logger/latest/env_logger/#enabling-logging).
