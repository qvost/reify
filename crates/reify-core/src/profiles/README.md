# Pre-warmed wire profiles

The embedded catalog contains 25 services and 39 read operations. Profiles are
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

| Service | Matched domains (aliases) | Pre-warmed endpoints |
| --- | --- | --- |
| arXiv | `arxiv.org` (`export.arxiv.org`) | `GET /api/query` (`search_query`, `max_results`) |
| CoinGecko | `coingecko.com` (`api.coingecko.com`) | `GET /api/v3/simple/price` (`ids`, `vs_currencies`); `GET /api/v3/search` (`query`) |
| Crates.io | `crates.io` (`api.crates.io`) | `GET /api/v1/crates` (`q`, `per_page`); `GET /api/v1/crates/{name}` |
| DuckDuckGo Instant Answer | `duckduckgo.com` (`api.duckduckgo.com`) | `GET /?format=json` (`q`) |
| DummyJSON | `dummyjson.com` | `GET /products` (`limit`, `skip`); `GET /products/search` (`q`) |
| Frankfurter FX Rates | `frankfurter.app` (`api.frankfurter.app`) | `GET /latest` (`from`, `to`) |
| GitHub | `github.com` (`api.github.com`) | `GET /search/repositories`; `GET /repos/{owner}/{repo}`; `GET /repos/{owner}/{repo}/issues` |
| GitLab | `gitlab.com` | `GET /api/v4/projects` (`search`) |
| Gutendex | `gutendex.com` | `GET /books/` (`search`) |
| Hacker News | `news.ycombinator.com` (`hn.algolia.com`) | `GET /api/v1/search` (`q`, `tags`, `page`); `GET /v0/item/{id}.json` |
| JSONPlaceholder | `jsonplaceholder.typicode.com` | `GET /posts` (`userId`); `GET /posts/{id}` |
| Linear | `linear.app` (`api.linear.app`) | `POST /graphql` (issue search, team listing) |
| npm Registry | `npmjs.com` (`registry.npmjs.org`, `registry.npmjs.com`) | `GET /-/v1/search` (`text`, `size`); `GET /{package}` |
| Open-Elevation | `open-elevation.com` (`api.open-elevation.com`) | `GET /api/v1/lookup` (`locations`) |
| Open Library | `openlibrary.org` | `GET /search.json` (`q`) |
| Open-Meteo | `open-meteo.com` (`api.open-meteo.com`) | `GET /v1/forecast` (`latitude`, `longitude`, `current_weather`) |
| PokeAPI | `pokeapi.co` (`api.pokeapi.co`) | `GET /api/v2/pokemon/{name_or_id}`; `GET /api/v2/pokemon` (`limit`, `offset`) |
| PyPI | `pypi.org` (`pypi.python.org`) | `GET /pypi/{project}/json` |
| Reddit | `reddit.com` (`www.reddit.com`, `oauth.reddit.com`) | `GET /r/{subreddit}/{sort}.json`; `GET /search.json` (`q`) |
| REST Countries | `restcountries.com` | `GET /v3.1/name/{name}`; `GET /v3.1/all` |
| Semantic Scholar | `semanticscholar.org` (`api.semanticscholar.org`) | `GET /graph/v1/paper/search` (`query`, `limit`) |
| Stack Exchange | `stackoverflow.com` (`api.stackexchange.com`) | `GET /2.3/search/advanced?site=stackoverflow` (`q`) |
| Stripe | `stripe.com` (`api.stripe.com`) | `GET /v1/customers`; `GET /v1/charges` |
| Wikidata | `wikidata.org` (`www.wikidata.org`) | `GET /w/api.php?action=wbsearchentities&search={search}&language=en&format=json` |
| Wikipedia | `wikipedia.org` (`*.wikipedia.org`) | `GET /w/rest.php/v1/search/page` (`q`, `limit`); `GET /api/rest_v1/page/summary/{title}` |

The paths in the table are relative to the matching API origin; inputs in
parentheses are typed query parameters. The three supplemental profiles are
GitLab, Open Library, and Stack Exchange.

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
- Crates.io: [API data access](https://crates.io/data-access).
- npm: [registry API](https://github.com/npm/registry/blob/master/docs/REGISTRY-API.md).
- PyPI: [JSON API](https://docs.pypi.org/api/json/).
- arXiv: [API user manual](https://info.arxiv.org/help/api/user-manual.html).
- CoinGecko: [API reference](https://docs.coingecko.com/reference/introduction).
- Frankfurter: [API documentation](https://www.frankfurter.app/docs/).
- REST Countries: [API](https://restcountries.com/).
- PokeAPI: [version 2 documentation](https://pokeapi.co/docs/v2).
- JSONPlaceholder: [guide](https://jsonplaceholder.typicode.com/guide/).
- Gutendex: [API](https://gutendex.com/).
- Semantic Scholar: [API documentation](https://api.semanticscholar.org/api-docs/).
- DuckDuckGo: [Instant Answer API](https://duckduckgo.com/api).
- Open-Elevation: [project and API](https://github.com/Jorl17/open-elevation).
- Wikidata: [entity search API](https://www.wikidata.org/w/api.php?action=help&modules=wbsearchentities).
- GitLab: [projects API](https://docs.gitlab.com/api/projects/).
- Open Library: [developer API](https://openlibrary.org/developers/api).
- Stack Exchange: [search/advanced API](https://api.stackexchange.com/docs/search-advanced).

Upstream access, authentication, availability, and rate limits still apply when
an agent executes a tool. A pre-warmed mount does not verify the live service.
