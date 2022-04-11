# iroha-chat
## Intro and setup
This package follows closely the spec given in the [test task](https://hackmd.io/@r3XngjBBSumx2rU-hKU7Qg/BkbHS80cv).
As such, the binary accepts two mandatory arguments, message period `--period` and port `--port` that will be used
as a name (in our implementation ports are given out randomly, so this field is just a facade), as well as a third 
optional one, `--connect`, which specifies the parent port to connect to. The arguments can be passed after
`cargo run --` in the command line, or after first running `cargo build` and then
calling the binary as `target/debug/iroha-chat` and passing arguments directly.

Additionally, for reviewing the snapshots from the tests provided by the `insta` crate, one might install 
`cargo-insta` package by running `cargo install cargo-insta` or using their [vscode extension](https://insta.rs/docs/vscode/)
but that is not strictly necessary.

## Architecture
CLI scaffolding is done via `clap` and error handling via `anyhow` for a fast, battery-included experience.

The chat itself runs on WebSocket implementation provided by `tungstenite` and its async wrapper for `tokio`.
In the development process using gRPC calls was considered as an option, but that direction encountered
problems with lifetimes of streams passed into and from the server via a `StreamMap`, so a more down-to-earth
approach was taken in the end, as bidirectional streaming over HTTP provides a perfect abstraction for such
applications.

The only source of state in the app is a `PeerMap`, which is
an arc-wrapped hashmap with a mutex, keeping peer addresses as
keys and their sender parts of the WS channel as values. It is
updated with arrival and departure of new peers.

Message sending is done by ticking of `tokio::time::interval()`
future inside a `select!` macro.

The first client is created by the server and connected with the
`handle_client()` function, which helps avoid weird juggling of
intervals inside a shared server struct (another approach that
didn't prove to be fruitful).

Many of the expected use cases are handled more or less correctly. As such, server disconnect with a `SIGINT` gracefully sends
closing messages to peers, just as peer disconnect does.

Testing was another ponder point. Technically the most correct
but cumbersome approach would be to collect the logs for many
scenarios (just one peer spawned by server, a few peers, peers disconnecting after a period, server disconnecting) via integration testing and check various invariants, preferrably
fuzzing the inputs too, such as connect and disconnect times,
but the time scope didn't allow for that.

For example, even trying to collect outputs after one client
has disconnected already presented some difficulties: the `assert-cmd` crate's wrapper over `std::process::Command`
didn't provide the option of getting child's id, as I needed to use something like `nix` to send a proper `SIGINT` instead of a `SIGKILL` to see the termination logic. Then when I tried
to collect the outputs, it all kept getting jumbled up and all hell broke loose.

So instead, I went for snapshot
testing providing a simpler alternative: after one failed run of
the tests, a snapshot of the results is generated, and it can be accepted or rejected, as well as inlined into the test (as it was done), saved as a file or serialized. As such, even though our log outputs are complex and ever-changing, one can always resort to
a fallible but appealing method of visual inspection of captured
logs. I do understand the imperfection of this approach, but it's definitely better than nothing.

## Further directions
The most crucial improvement that could be done is testing, as already mentioned above.

Another one could be more structured code, with common use
functions factored out into `lib.rs` and maybe separate binaries for both server and client which would promote reusability. Another point here could be making a deeper chat service type that would hold its state and do all the client handling logic,
instead of having that as disjoint functions.

Logging could be another subpar point in the implementation. The `simplelog` crate that was used has good defaults, but doesn't
boast great flexibility, so if great amounts of logs need
to be saved with rotation or the like, it would be better to
use something like `log4rs`.

As for error handling, there are still some `.unwrap()`s scattered here and there, and additional thought is needed to determine whether panics in those places would be acceptable in
production code. 

Finally, the CLI chat itself is obviously very feature poor,
only sending random messages and giving no control to the user. Some additional functionality could be added, such as capturing `stdin`, sending private messages or muting users, creating channels and so on and so on... Sky is the limit! 