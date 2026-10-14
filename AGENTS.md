# Release notes for contract developers

Apply this recipe to public release descriptions and `docs/release-notes.md`:

1. Write in English for contract developers. A release body should normally be
   at most 250 words, excluding a short command snippet. Start with the version
   and one literal sentence describing the useful change. Explain what changed,
   what developers can do now, why it matters, what they need and where examples
   live. Do not open with an audit or provenance ledger.
2. Use a compact structure: **What's Changed** (at most four bullets explaining
   practical before/after), **Get Started** (requirements and a link or tested
   command), **Examples** (exact useful examples), and **Compatibility**
   (actionable breaking changes and known limitations). Omit empty sections.
   Use normal Markdown headings, short paragraphs, inline code and useful links.
   Use labels such as Breaking or Known limitation only when accurate. No
   decorative badges, raw SHA lists, gate counts, internal review stories or
   marketing.
3. Distinguish new functionality from existing functionality that was corrected
   or tested. Claim a fix or supported capability only when the released version
   has evidence for that exact behavior. Link unresolved user-facing issues under
   known limitations; revisit their status for every release.
4. Preserve essential migration and platform limitations. Link to existing
   documentation, manifests and receipts for detailed provenance, test
   inventories, internal review and hashes; never delete required evidence to
   shorten prose. Keep local development-state migration guidance distinct from
   production operations, and retain applicable license scopes.
5. Verify links, exact example names and commands, installed prerequisites and
   delivered versions against the public `README.md`, `examples/README.md`,
   `docs/getting-started.md` and release manifests/receipts. Published immutable
   archives, tags and assets stay unchanged for a notes-only correction.
