# Upstream notice inputs

These unmodified texts cover dependency crates whose published archives omit a
license file. `index.json` binds each exact Cargo package identity, original
license expression, repository metadata, `.cargo_vcs_info.json` commit and path
to the license files from that same upstream commit, their SHA-256 and URLs.
They are build inputs, not additional AVM license grants. The package builder
checks those bindings and the actual registry archive/source hashes against
the committed runtime lock before recording the dependency notices.

Each grant keeps its upstream scope and alternatives. The SDK MIT grant does
not replace them or the separate Acki Nacki node owner grant. Changing a
dependency commit or license requires reviewing its notice inputs explicitly;
the builder never substitutes a generic license text based only on an SPDX name.
