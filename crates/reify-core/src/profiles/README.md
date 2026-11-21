# Pre-warmed wire profiles

The embedded catalog contains 8 services and 16 read operations. Profiles are
curated request plans, with `prewarmed` provenance; they are not browser captures
or stored authenticated sessions. No network requests occur during selection or
mounting, and no credentials ship in the catalog.

```sh
reify profiles
reify https://news.ycombinator.com 'search stories'
reify --fresh https://news.ycombinator.com 'search stories'
reify serve --no-profile
```

Domain matching uses complete DNS labels, including aliases and subdomains.
An explicit `*.` alias excludes the bare domain. Non-HTTP(S) URLs, URL credentials,
and nonstandard ports do not match. A goal selects the tools with the most
matching keywords; an unrelated goal falls through to live discovery. Without a
goal, all tools for the service are mounted.

The normal order is the process cache, then a matching embedded profile, then
SSR/Chromium discovery. `--fresh` and its `--no-profile` alias bypass profiles.
The MCP bootstrap tool also accepts `fresh: true` to replace a cached generation
with live discovery; `reify call --fresh --gateway …` sends that option remotely.
Drift recovery always uses live discovery rather than reinstalling a stale plan.
`reify probe` remains a live wire diagnostic regardless of profiles.

Tools enter `CachedApp` with the usual five-minute TTL. On process startup,
`warm_profiles()` validates the catalog and prepares request validators and MCP
wire definitions once. Subsequent empty-cache mounts clone those immutable plans.
The reported profile match duration covers selection and app construction;
it excludes process/runtime startup, initialization, and binding the listening
socket. Measure the complete server mount separately with:

```sh
cargo run --release -p reify-server --example profile_mount
```

This benchmark reports initialization separately, then 1,000 independent
empty-cache mounts, with the browser executable deliberately nonexistent.
The server integration test also checks a complete in-process profile mount
against a 1 ms budget after initialization. Hardware and scheduling affect
elapsed time; no fixed duration is printed as a measured result.

## Endpoints and credentials

The API origin may differ from the requested website. Credentials are resolved
for the actual rendered API URL at execution time. Linear and Stripe require an
Authorization header; GitHub private data also needs API-authorized credentials.
A website login cookie does not become an API token or cross origin boundaries.
Wikipedia's embedded plans use English Wikipedia, including when selected via
another Wikipedia subdomain. Use `--fresh` for discovery on a different edition
or for an operation outside the catalog.

The catalog follows these upstream API references:

- Hacker News: [Algolia search](https://hn.algolia.com/api) and
  [official item API](https://github.com/HackerNews/API).
- GitHub: [search](https://docs.github.com/en/rest/search),
  [repositories](https://docs.github.com/en/rest/repos/repos), and
  [repository issues](https://docs.github.com/en/rest/issues/issues).
- Linear: [GraphQL](https://linear.app/developers/graphql) and
  [filtering](https://linear.app/developers/filtering). Issue search uses a
  case-insensitive title/description filter and typed GraphQL variables.
- DummyJSON: [products](https://dummyjson.com/docs/products).
- Wikipedia: [REST search](https://www.mediawiki.org/wiki/API:REST_API/Reference)
  and [page summaries](https://www.mediawiki.org/wiki/Wikimedia_REST_API).
- Open-Meteo: [forecast API](https://open-meteo.com/en/docs).
- Reddit: [listings and search](https://www.reddit.com/dev/api/).
- Stripe: [customers](https://docs.stripe.com/api/customers/list) and
  [charges](https://docs.stripe.com/api/charges/list).

Upstream access, authentication, availability, and rate limits still apply when
an agent executes a tool. A pre-warmed mount does not verify the live service.
