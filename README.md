<h1><img src=".github/readme-icon.png" width="44" height="44" align="absmiddle" alt=""> Open RotMG / Server</h1>

Rust backend for the Realm of the Mad God 2012 (123.5.1) client.

See https://openrotmg.org/restoration/ to see what's completed.


## Run locally

Install Rust/Cargo and Python 3, then run. The client downloads automatically:

```sh
./run.sh 8080
```

Open the URL printed when the server is ready.

`realm_count` (2 or 3, default 2) is a maintained-lineage ceiling, not a startup count. Startup creates one Realm; sustained 80% occupancy/reservations for 30 simulation seconds in every open Realm grows one. An extra empty, dependency-free Realm retires after five minutes with another open Realm remaining and all remaining opens at most 60% occupied. These thresholds are reconstruction choices. Replacements remain one-for-one; dependent predecessor worlds can temporarily exceed the physical-world ceiling.

---

Join Discord: https://discord.gg/EwZWNzzYwP
