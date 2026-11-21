//! Curated endpoint definitions; see README.md for upstream references.
use super::{PrewarmedParameter, PrewarmedProfile, PrewarmedTool};

const fn p(
    name: &'static str,
    kind: &'static str,
    description: &'static str,
    required: bool,
) -> PrewarmedParameter {
    PrewarmedParameter {
        name,
        kind,
        description,
        required,
    }
}

const HN_SEARCH: &[PrewarmedParameter] = &[
    p("q", "string", "Text to find in stories and comments.", true),
    p("tags", "string", "Optional Algolia tag filter.", false),
    p("page", "integer", "Zero-based result page.", false),
];
const HN_ITEM: &[PrewarmedParameter] = &[p("id", "integer", "Hacker News item ID.", true)];
const GITHUB_SEARCH: &[PrewarmedParameter] = &[
    p("q", "string", "GitHub repository search query.", true),
    p(
        "sort",
        "string",
        "Sort order, such as stars or updated.",
        false,
    ),
];
const REPOSITORY: &[PrewarmedParameter] = &[
    p("owner", "string", "Repository owner login.", true),
    p("repo", "string", "Repository name.", true),
];
const LINEAR_SEARCH: &[PrewarmedParameter] = &[p(
    "query",
    "string",
    "Text to search in Linear issues.",
    true,
)];
const PRODUCTS: &[PrewarmedParameter] = &[
    p("limit", "integer", "Maximum number of products.", false),
    p("skip", "integer", "Number of products to skip.", false),
];
const PRODUCT_SEARCH: &[PrewarmedParameter] =
    &[p("q", "string", "Text to search in product names.", true)];
const WIKI_SEARCH: &[PrewarmedParameter] = &[
    p("query", "string", "Text to search in article titles.", true),
    p(
        "limit",
        "integer",
        "Maximum number of matching pages.",
        false,
    ),
];
const WIKI_TITLE: &[PrewarmedParameter] = &[p("title", "string", "Wikipedia page title.", true)];
const FORECAST: &[PrewarmedParameter] = &[
    p("latitude", "number", "Latitude in WGS84 coordinates.", true),
    p(
        "longitude",
        "number",
        "Longitude in WGS84 coordinates.",
        true,
    ),
    p(
        "current_weather",
        "boolean",
        "Include current weather conditions.",
        false,
    ),
];
const REDDIT_POSTS: &[PrewarmedParameter] = &[
    p("subreddit", "string", "Subreddit name without r/.", true),
    p("sort", "string", "Listing sort: hot, new, or top.", false),
];
const REDDIT_SEARCH: &[PrewarmedParameter] =
    &[p("q", "string", "Text to search in Reddit posts.", true)];
const STRIPE_LIST: &[PrewarmedParameter] =
    &[p("limit", "integer", "Maximum number of records.", false)];
const CRATES_SEARCH: &[PrewarmedParameter] = &[
    p(
        "q",
        "string",
        "Text to search in crate names and descriptions.",
        true,
    ),
    p(
        "per_page",
        "integer",
        "Maximum number of crates to return.",
        false,
    ),
];
const CRATE_NAME: &[PrewarmedParameter] = &[p("name", "string", "Crate name.", true)];
const NPM_SEARCH: &[PrewarmedParameter] = &[
    p("text", "string", "Text to search in npm packages.", true),
    p(
        "size",
        "integer",
        "Maximum number of packages to return.",
        false,
    ),
];
const NPM_PACKAGE: &[PrewarmedParameter] = &[p("package", "string", "npm package name.", true)];
const PYPI_PROJECT: &[PrewarmedParameter] = &[p("project", "string", "PyPI project name.", true)];
const ARXIV_QUERY: &[PrewarmedParameter] = &[
    p("search_query", "string", "arXiv query expression.", true),
    p(
        "max_results",
        "integer",
        "Maximum number of papers to return.",
        false,
    ),
];
const COIN_PRICE: &[PrewarmedParameter] = &[
    p("ids", "string", "Comma-separated CoinGecko coin IDs.", true),
    p(
        "vs_currencies",
        "string",
        "Comma-separated quote currencies.",
        true,
    ),
];
const COIN_SEARCH: &[PrewarmedParameter] =
    &[p("query", "string", "Text to search for coins.", true)];
const FX_RATES: &[PrewarmedParameter] = &[
    p("from", "string", "Base currency code.", false),
    p(
        "to",
        "string",
        "Comma-separated target currency codes.",
        false,
    ),
];
const COUNTRY_NAME: &[PrewarmedParameter] = &[p("name", "string", "Country name.", true)];
const POKEMON_NAME: &[PrewarmedParameter] =
    &[p("name_or_id", "string", "Pokémon name or ID.", true)];
const POKEMON_LIST: &[PrewarmedParameter] = &[
    p(
        "limit",
        "integer",
        "Maximum number of Pokémon to return.",
        false,
    ),
    p("offset", "integer", "Number of Pokémon to skip.", false),
];
const POST_USER: &[PrewarmedParameter] = &[p("userId", "integer", "Filter by user ID.", false)];
const POST_ID: &[PrewarmedParameter] = &[p("id", "integer", "Post ID.", true)];
const BOOK_SEARCH: &[PrewarmedParameter] = &[p(
    "search",
    "string",
    "Text to search in book metadata.",
    true,
)];
const PAPER_SEARCH: &[PrewarmedParameter] = &[
    p(
        "query",
        "string",
        "Text to search in paper titles and abstracts.",
        true,
    ),
    p(
        "limit",
        "integer",
        "Maximum number of papers to return.",
        false,
    ),
];
const DDG_QUERY: &[PrewarmedParameter] = &[p("q", "string", "Instant-answer search query.", true)];
const ELEVATION_LOCATIONS: &[PrewarmedParameter] = &[p(
    "locations",
    "string",
    "JSON array of latitude/longitude coordinate objects.",
    true,
)];
const WIKIDATA_SEARCH: &[PrewarmedParameter] = &[p(
    "search",
    "string",
    "Text to search for Wikidata entities.",
    true,
)];
const GITLAB_SEARCH: &[PrewarmedParameter] = &[p(
    "search",
    "string",
    "Text to search in GitLab projects.",
    true,
)];
const OPEN_LIBRARY_SEARCH: &[PrewarmedParameter] = &[p(
    "q",
    "string",
    "Text to search in Open Library works.",
    true,
)];
const STACK_OVERFLOW_SEARCH: &[PrewarmedParameter] = &[p(
    "q",
    "string",
    "Text to search in Stack Overflow questions.",
    true,
)];
const HN_TOOLS: &[PrewarmedTool] = &[
    PrewarmedTool {
        name: "search_stories",
        description: "Search Hacker News stories and comments using Algolia.",
        method: "GET",
        endpoint: "https://hn.algolia.com/api/v1/search",
        path_parameters: &[],
        query_parameters: &[("q", "query"), ("tags", "tags"), ("page", "page")],
        keywords: &["search", "story", "stories", "news", "comments"],
        parameters: HN_SEARCH,
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &[],
    },
    PrewarmedTool {
        name: "get_item",
        description: "Fetch a Hacker News item by its numeric ID.",
        method: "GET",
        endpoint: "https://hacker-news.firebaseio.com/v0/item/{id}.json",
        path_parameters: &["id"],
        query_parameters: &[],
        keywords: &["item", "detail", "lookup"],
        parameters: HN_ITEM,
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &[],
    },
];
const GITHUB_TOOLS: &[PrewarmedTool] = &[
    PrewarmedTool {
        name: "search_repositories",
        description: "Search public and accessible GitHub repositories.",
        method: "GET",
        endpoint: "https://api.github.com/search/repositories",
        path_parameters: &[],
        query_parameters: &[("q", "q"), ("sort", "sort")],
        keywords: &["search", "repository", "repositories", "repo"],
        parameters: GITHUB_SEARCH,
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &[],
    },
    PrewarmedTool {
        name: "get_repository",
        description: "Fetch repository metadata by owner and repository name.",
        method: "GET",
        endpoint: "https://api.github.com/repos/{owner}/{repo}",
        path_parameters: &["owner", "repo"],
        query_parameters: &[],
        keywords: &["repository", "repo", "details", "metadata"],
        parameters: REPOSITORY,
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &[],
    },
    PrewarmedTool {
        name: "list_issues",
        description: "List issues and pull requests in a GitHub repository.",
        method: "GET",
        endpoint: "https://api.github.com/repos/{owner}/{repo}/issues",
        path_parameters: &["owner", "repo"],
        query_parameters: &[],
        keywords: &["issue", "issues", "pull", "requests"],
        parameters: REPOSITORY,
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &[],
    },
];
const LINEAR_SEARCH_BODY: &str = r#"{"query":"query SearchIssues($query: String!) { issues(filter: { or: [{ title: { containsIgnoreCase: $query } }, { description: { containsIgnoreCase: $query } }] }) { nodes { id identifier title url } } }","variables":{"query":"{query}"}}"#;
const LINEAR_TEAMS_BODY: &str = r#"{"query":"query { teams { nodes { id name key } } }"}"#;
const LINEAR_TOOLS: &[PrewarmedTool] = &[
    PrewarmedTool {
        name: "search_issues",
        description: "Search Linear issues by text through its GraphQL API.",
        method: "POST",
        endpoint: "https://api.linear.app/graphql",
        path_parameters: &[],
        query_parameters: &[],
        keywords: &["search", "issue", "issues", "ticket", "tickets"],
        parameters: LINEAR_SEARCH,
        body_template: Some(LINEAR_SEARCH_BODY),
        path_defaults: &[],
        required_auth_keys: &["authorization"],
    },
    PrewarmedTool {
        name: "get_teams",
        description: "List teams available to the authenticated Linear account.",
        method: "POST",
        endpoint: "https://api.linear.app/graphql",
        path_parameters: &[],
        query_parameters: &[],
        keywords: &["team", "teams"],
        parameters: &[],
        body_template: Some(LINEAR_TEAMS_BODY),
        path_defaults: &[],
        required_auth_keys: &["authorization"],
    },
];
const DUMMYJSON_TOOLS: &[PrewarmedTool] = &[
    PrewarmedTool {
        name: "get_products",
        description: "List products from the DummyJSON sample catalog.",
        method: "GET",
        endpoint: "https://dummyjson.com/products",
        path_parameters: &[],
        query_parameters: &[("limit", "limit"), ("skip", "skip")],
        keywords: &["get", "list", "product", "products", "catalog"],
        parameters: PRODUCTS,
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &[],
    },
    PrewarmedTool {
        name: "search_products",
        description: "Search products by text in the DummyJSON sample catalog.",
        method: "GET",
        endpoint: "https://dummyjson.com/products/search",
        path_parameters: &[],
        query_parameters: &[("q", "q")],
        keywords: &["search", "product", "products", "find"],
        parameters: PRODUCT_SEARCH,
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &[],
    },
];
const WIKIPEDIA_TOOLS: &[PrewarmedTool] = &[
    PrewarmedTool {
        name: "search_articles",
        description: "Search English Wikipedia pages by title.",
        method: "GET",
        endpoint: "https://en.wikipedia.org/w/rest.php/v1/search/page",
        path_parameters: &[],
        query_parameters: &[("query", "q"), ("limit", "limit")],
        keywords: &["search", "article", "articles", "wikipedia", "page"],
        parameters: WIKI_SEARCH,
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &[],
    },
    PrewarmedTool {
        name: "get_page_summary",
        description: "Fetch the English Wikipedia summary for a page title.",
        method: "GET",
        endpoint: "https://en.wikipedia.org/api/rest_v1/page/summary/{title}",
        path_parameters: &["title"],
        query_parameters: &[],
        keywords: &["summary", "article", "page", "title", "wikipedia"],
        parameters: WIKI_TITLE,
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &[],
    },
];
const WEATHER_TOOLS: &[PrewarmedTool] = &[PrewarmedTool {
    name: "get_forecast",
    description: "Get an Open-Meteo forecast for WGS84 coordinates.",
    method: "GET",
    endpoint: "https://api.open-meteo.com/v1/forecast",
    path_parameters: &[],
    query_parameters: &[
        ("latitude", "latitude"),
        ("longitude", "longitude"),
        ("current_weather", "current_weather"),
    ],
    keywords: &["weather", "forecast", "temperature", "climate"],
    parameters: FORECAST,
    body_template: None,
    path_defaults: &[],
    required_auth_keys: &[],
}];
const REDDIT_TOOLS: &[PrewarmedTool] = &[
    PrewarmedTool {
        name: "get_subreddit_posts",
        description: "List posts from a subreddit, optionally by sort order.",
        method: "GET",
        endpoint: "https://www.reddit.com/r/{subreddit}/{sort}.json",
        path_parameters: &["subreddit", "sort"],
        query_parameters: &[],
        keywords: &["subreddit", "posts", "listing", "hot", "new", "top"],
        parameters: REDDIT_POSTS,
        body_template: None,
        path_defaults: &[("sort", "hot")],
        required_auth_keys: &[],
    },
    PrewarmedTool {
        name: "search_posts",
        description: "Search Reddit posts by text.",
        method: "GET",
        endpoint: "https://www.reddit.com/search.json",
        path_parameters: &[],
        query_parameters: &[("q", "q")],
        keywords: &["search", "posts", "post", "find"],
        parameters: REDDIT_SEARCH,
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &[],
    },
];
const STRIPE_TOOLS: &[PrewarmedTool] = &[
    PrewarmedTool {
        name: "list_customers",
        description: "List customers in the configured Stripe account.",
        method: "GET",
        endpoint: "https://api.stripe.com/v1/customers",
        path_parameters: &[],
        query_parameters: &[("limit", "limit")],
        keywords: &["list", "customer", "customers"],
        parameters: STRIPE_LIST,
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &["authorization"],
    },
    PrewarmedTool {
        name: "list_charges",
        description: "List charges in the configured Stripe account.",
        method: "GET",
        endpoint: "https://api.stripe.com/v1/charges",
        path_parameters: &[],
        query_parameters: &[("limit", "limit")],
        keywords: &["list", "charge", "charges", "payment"],
        parameters: STRIPE_LIST,
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &["authorization"],
    },
];

const ARXIV_TOOLS: &[PrewarmedTool] = &[PrewarmedTool {
    name: "query_papers",
    description: "Search arXiv papers with an arXiv query expression.",
    method: "GET",
    endpoint: "https://export.arxiv.org/api/query",
    path_parameters: &[],
    query_parameters: &[
        ("search_query", "search_query"),
        ("max_results", "max_results"),
    ],
    keywords: &["search", "query", "paper", "papers", "arxiv"],
    parameters: ARXIV_QUERY,
    body_template: None,
    path_defaults: &[],
    required_auth_keys: &[],
}];
const COINGECKO_TOOLS: &[PrewarmedTool] = &[
    PrewarmedTool {
        name: "get_coin_price",
        description: "Get CoinGecko spot prices in selected quote currencies.",
        method: "GET",
        endpoint: "https://api.coingecko.com/api/v3/simple/price",
        path_parameters: &[],
        query_parameters: &[("ids", "ids"), ("vs_currencies", "vs_currencies")],
        keywords: &["price", "coin", "coins", "crypto", "currency"],
        parameters: COIN_PRICE,
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &[],
    },
    PrewarmedTool {
        name: "search_coins",
        description: "Search CoinGecko for coins by name or symbol.",
        method: "GET",
        endpoint: "https://api.coingecko.com/api/v3/search",
        path_parameters: &[],
        query_parameters: &[("query", "query")],
        keywords: &["search", "coin", "coins", "crypto"],
        parameters: COIN_SEARCH,
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &[],
    },
];
const CRATES_TOOLS: &[PrewarmedTool] = &[
    PrewarmedTool {
        name: "search_crates",
        description: "Search crates.io for Rust packages.",
        method: "GET",
        endpoint: "https://crates.io/api/v1/crates",
        path_parameters: &[],
        query_parameters: &[("q", "q"), ("per_page", "per_page")],
        keywords: &["search", "crate", "crates", "rust", "package"],
        parameters: CRATES_SEARCH,
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &[],
    },
    PrewarmedTool {
        name: "get_crate",
        description: "Fetch crates.io metadata for a Rust package.",
        method: "GET",
        endpoint: "https://crates.io/api/v1/crates/{name}",
        path_parameters: &["name"],
        query_parameters: &[],
        keywords: &["get", "crate", "crates", "package", "details"],
        parameters: CRATE_NAME,
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &[],
    },
];
const DUCKDUCKGO_TOOLS: &[PrewarmedTool] = &[PrewarmedTool {
    name: "instant_answer",
    description: "Fetch a DuckDuckGo Instant Answer response.",
    method: "GET",
    endpoint: "https://api.duckduckgo.com/?format=json",
    path_parameters: &[],
    query_parameters: &[("q", "q")],
    keywords: &["search", "answer", "instant", "duckduckgo"],
    parameters: DDG_QUERY,
    body_template: None,
    path_defaults: &[],
    required_auth_keys: &[],
}];
const FRANKFURTER_TOOLS: &[PrewarmedTool] = &[PrewarmedTool {
    name: "latest_rates",
    description: "Get the latest Frankfurter foreign exchange rates.",
    method: "GET",
    endpoint: "https://api.frankfurter.app/latest",
    path_parameters: &[],
    query_parameters: &[("from", "from"), ("to", "to")],
    keywords: &["rate", "rates", "exchange", "currency", "forex"],
    parameters: FX_RATES,
    body_template: None,
    path_defaults: &[],
    required_auth_keys: &[],
}];
const GITLAB_TOOLS: &[PrewarmedTool] = &[PrewarmedTool {
    name: "search_projects",
    description: "Search public GitLab projects by text.",
    method: "GET",
    endpoint: "https://gitlab.com/api/v4/projects",
    path_parameters: &[],
    query_parameters: &[("search", "search")],
    keywords: &["search", "project", "projects", "gitlab", "repository"],
    parameters: GITLAB_SEARCH,
    body_template: None,
    path_defaults: &[],
    required_auth_keys: &[],
}];
const GUTENDEX_TOOLS: &[PrewarmedTool] = &[PrewarmedTool {
    name: "search_books",
    description: "Search Project Gutenberg books by metadata text.",
    method: "GET",
    endpoint: "https://gutendex.com/books/",
    path_parameters: &[],
    query_parameters: &[("search", "search")],
    keywords: &["search", "book", "books", "gutenberg", "author"],
    parameters: BOOK_SEARCH,
    body_template: None,
    path_defaults: &[],
    required_auth_keys: &[],
}];
const JSONPLACEHOLDER_TOOLS: &[PrewarmedTool] = &[
    PrewarmedTool {
        name: "list_posts",
        description: "List JSONPlaceholder posts, optionally filtered by user ID.",
        method: "GET",
        endpoint: "https://jsonplaceholder.typicode.com/posts",
        path_parameters: &[],
        query_parameters: &[("userId", "userId")],
        keywords: &["list", "posts", "post", "user"],
        parameters: POST_USER,
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &[],
    },
    PrewarmedTool {
        name: "get_post",
        description: "Fetch a JSONPlaceholder post by its numeric ID.",
        method: "GET",
        endpoint: "https://jsonplaceholder.typicode.com/posts/{id}",
        path_parameters: &["id"],
        query_parameters: &[],
        keywords: &["get", "post", "details", "lookup"],
        parameters: POST_ID,
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &[],
    },
];
const NPM_TOOLS: &[PrewarmedTool] = &[
    PrewarmedTool {
        name: "search_packages",
        description: "Search the npm registry for JavaScript packages.",
        method: "GET",
        endpoint: "https://registry.npmjs.org/-/v1/search",
        path_parameters: &[],
        query_parameters: &[("text", "text"), ("size", "size")],
        keywords: &["search", "package", "packages", "npm", "javascript"],
        parameters: NPM_SEARCH,
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &[],
    },
    PrewarmedTool {
        name: "get_package",
        description: "Fetch npm registry metadata for a package.",
        method: "GET",
        endpoint: "https://registry.npmjs.org/{package}",
        path_parameters: &["package"],
        query_parameters: &[],
        keywords: &["get", "package", "npm", "details", "metadata"],
        parameters: NPM_PACKAGE,
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &[],
    },
];
const OPEN_LIBRARY_TOOLS: &[PrewarmedTool] = &[PrewarmedTool {
    name: "search_works",
    description: "Search Open Library works by title, author, or text.",
    method: "GET",
    endpoint: "https://openlibrary.org/search.json",
    path_parameters: &[],
    query_parameters: &[("q", "q")],
    keywords: &["search", "book", "books", "work", "works", "library"],
    parameters: OPEN_LIBRARY_SEARCH,
    body_template: None,
    path_defaults: &[],
    required_auth_keys: &[],
}];
const OPEN_ELEVATION_TOOLS: &[PrewarmedTool] = &[PrewarmedTool {
    name: "lookup_elevation",
    description: "Look up elevations for one or more coordinates.",
    method: "GET",
    endpoint: "https://api.open-elevation.com/api/v1/lookup",
    path_parameters: &[],
    query_parameters: &[("locations", "locations")],
    keywords: &["lookup", "elevation", "altitude", "height", "coordinates"],
    parameters: ELEVATION_LOCATIONS,
    body_template: None,
    path_defaults: &[],
    required_auth_keys: &[],
}];
const PYPI_TOOLS: &[PrewarmedTool] = &[PrewarmedTool {
    name: "get_package_info",
    description: "Fetch Python package metadata from PyPI.",
    method: "GET",
    endpoint: "https://pypi.org/pypi/{project}/json",
    path_parameters: &["project"],
    query_parameters: &[],
    keywords: &["get", "package", "python", "pypi", "metadata"],
    parameters: PYPI_PROJECT,
    body_template: None,
    path_defaults: &[],
    required_auth_keys: &[],
}];
const POKEMON_TOOLS: &[PrewarmedTool] = &[
    PrewarmedTool {
        name: "get_pokemon",
        description: "Fetch Pokémon data by name or numeric ID.",
        method: "GET",
        endpoint: "https://pokeapi.co/api/v2/pokemon/{name_or_id}",
        path_parameters: &["name_or_id"],
        query_parameters: &[],
        keywords: &["get", "pokemon", "creature", "details"],
        parameters: POKEMON_NAME,
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &[],
    },
    PrewarmedTool {
        name: "list_pokemon",
        description: "List Pokémon with optional pagination.",
        method: "GET",
        endpoint: "https://pokeapi.co/api/v2/pokemon",
        path_parameters: &[],
        query_parameters: &[("limit", "limit"), ("offset", "offset")],
        keywords: &["list", "pokemon", "creatures"],
        parameters: POKEMON_LIST,
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &[],
    },
];
const REST_COUNTRIES_TOOLS: &[PrewarmedTool] = &[
    PrewarmedTool {
        name: "search_country",
        description: "Search REST Countries by country name.",
        method: "GET",
        endpoint: "https://restcountries.com/v3.1/name/{name}",
        path_parameters: &["name"],
        query_parameters: &[],
        keywords: &["search", "country", "countries", "nation"],
        parameters: COUNTRY_NAME,
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &[],
    },
    PrewarmedTool {
        name: "all_countries",
        description: "List all countries from REST Countries.",
        method: "GET",
        endpoint: "https://restcountries.com/v3.1/all",
        path_parameters: &[],
        query_parameters: &[],
        keywords: &["list", "all", "country", "countries", "nation"],
        parameters: &[],
        body_template: None,
        path_defaults: &[],
        required_auth_keys: &[],
    },
];
const SEMANTIC_SCHOLAR_TOOLS: &[PrewarmedTool] = &[PrewarmedTool {
    name: "search_papers",
    description: "Search Semantic Scholar papers by text.",
    method: "GET",
    endpoint: "https://api.semanticscholar.org/graph/v1/paper/search",
    path_parameters: &[],
    query_parameters: &[("query", "query"), ("limit", "limit")],
    keywords: &[
        "search", "paper", "papers", "semantic", "scholar", "research",
    ],
    parameters: PAPER_SEARCH,
    body_template: None,
    path_defaults: &[],
    required_auth_keys: &[],
}];
const STACK_EXCHANGE_TOOLS: &[PrewarmedTool] = &[PrewarmedTool {
    name: "search_questions",
    description: "Search Stack Overflow questions using the Stack Exchange API.",
    method: "GET",
    endpoint: "https://api.stackexchange.com/2.3/search/advanced?site=stackoverflow",
    path_parameters: &[],
    query_parameters: &[("q", "q")],
    keywords: &["search", "question", "questions", "stackoverflow", "stack"],
    parameters: STACK_OVERFLOW_SEARCH,
    body_template: None,
    path_defaults: &[],
    required_auth_keys: &[],
}];
const WIKIDATA_TOOLS: &[PrewarmedTool] = &[PrewarmedTool {
    name: "search_entities",
    description: "Search Wikidata entities in English.",
    method: "GET",
    endpoint: "https://www.wikidata.org/w/api.php?action=wbsearchentities&language=en&format=json",
    path_parameters: &[],
    query_parameters: &[("search", "search")],
    keywords: &["search", "entity", "entities", "wikidata", "knowledge"],
    parameters: WIKIDATA_SEARCH,
    body_template: None,
    path_defaults: &[],
    required_auth_keys: &[],
}];

/// All built-in service catalogs, kept in stable display order.
pub static PROFILES: &[PrewarmedProfile] = &[
    PrewarmedProfile {
        domain: "news.ycombinator.com",
        aliases: &["hn.algolia.com"],
        name: "Hacker News",
        description: "Hacker News story search and item lookup.",
        tools: HN_TOOLS,
    },
    PrewarmedProfile {
        domain: "github.com",
        aliases: &["api.github.com"],
        name: "GitHub",
        description: "GitHub repository and issue endpoints.",
        tools: GITHUB_TOOLS,
    },
    PrewarmedProfile {
        domain: "linear.app",
        aliases: &["api.linear.app"],
        name: "Linear",
        description: "Linear issue search and team listing.",
        tools: LINEAR_TOOLS,
    },
    PrewarmedProfile {
        domain: "dummyjson.com",
        aliases: &[],
        name: "DummyJSON",
        description: "Product listing and search from DummyJSON.",
        tools: DUMMYJSON_TOOLS,
    },
    PrewarmedProfile {
        domain: "wikipedia.org",
        aliases: &["*.wikipedia.org"],
        name: "Wikipedia",
        description: "Wikipedia page search and summaries.",
        tools: WIKIPEDIA_TOOLS,
    },
    PrewarmedProfile {
        domain: "open-meteo.com",
        aliases: &["api.open-meteo.com"],
        name: "Open-Meteo Weather",
        description: "Open-Meteo forecast endpoints.",
        tools: WEATHER_TOOLS,
    },
    PrewarmedProfile {
        domain: "reddit.com",
        aliases: &["www.reddit.com", "oauth.reddit.com"],
        name: "Reddit",
        description: "Reddit subreddit listings and post search.",
        tools: REDDIT_TOOLS,
    },
    PrewarmedProfile {
        domain: "stripe.com",
        aliases: &["api.stripe.com"],
        name: "Stripe",
        description: "Stripe customer and charge listings.",
        tools: STRIPE_TOOLS,
    },
    PrewarmedProfile {
        domain: "arxiv.org",
        aliases: &["export.arxiv.org"],
        name: "arXiv",
        description: "arXiv scholarly paper search.",
        tools: ARXIV_TOOLS,
    },
    PrewarmedProfile {
        domain: "coingecko.com",
        aliases: &["api.coingecko.com"],
        name: "CoinGecko",
        description: "Cryptocurrency price lookups and coin search.",
        tools: COINGECKO_TOOLS,
    },
    PrewarmedProfile {
        domain: "crates.io",
        aliases: &["api.crates.io"],
        name: "Crates.io",
        description: "Rust crate search and package metadata.",
        tools: CRATES_TOOLS,
    },
    PrewarmedProfile {
        domain: "duckduckgo.com",
        aliases: &["api.duckduckgo.com"],
        name: "DuckDuckGo Instant Answer",
        description: "DuckDuckGo instant-answer search.",
        tools: DUCKDUCKGO_TOOLS,
    },
    PrewarmedProfile {
        domain: "frankfurter.app",
        aliases: &["api.frankfurter.app"],
        name: "Frankfurter FX Rates",
        description: "Foreign exchange rates from Frankfurter.",
        tools: FRANKFURTER_TOOLS,
    },
    PrewarmedProfile {
        domain: "gitlab.com",
        aliases: &[],
        name: "GitLab",
        description: "Public GitLab project search.",
        tools: GITLAB_TOOLS,
    },
    PrewarmedProfile {
        domain: "gutendex.com",
        aliases: &[],
        name: "Gutendex",
        description: "Project Gutenberg book search.",
        tools: GUTENDEX_TOOLS,
    },
    PrewarmedProfile {
        domain: "jsonplaceholder.typicode.com",
        aliases: &[],
        name: "JSONPlaceholder",
        description: "Sample post listing and lookup endpoints.",
        tools: JSONPLACEHOLDER_TOOLS,
    },
    PrewarmedProfile {
        domain: "npmjs.com",
        aliases: &["registry.npmjs.org", "registry.npmjs.com"],
        name: "npm Registry",
        description: "npm package search and registry metadata.",
        tools: NPM_TOOLS,
    },
    PrewarmedProfile {
        domain: "open-elevation.com",
        aliases: &["api.open-elevation.com"],
        name: "Open-Elevation",
        description: "Coordinate elevation lookups.",
        tools: OPEN_ELEVATION_TOOLS,
    },
    PrewarmedProfile {
        domain: "openlibrary.org",
        aliases: &[],
        name: "Open Library",
        description: "Open Library work search.",
        tools: OPEN_LIBRARY_TOOLS,
    },
    PrewarmedProfile {
        domain: "pokeapi.co",
        aliases: &["api.pokeapi.co"],
        name: "PokeAPI",
        description: "Pokémon lookup and paginated listings.",
        tools: POKEMON_TOOLS,
    },
    PrewarmedProfile {
        domain: "pypi.org",
        aliases: &["pypi.python.org"],
        name: "PyPI",
        description: "Python package metadata.",
        tools: PYPI_TOOLS,
    },
    PrewarmedProfile {
        domain: "restcountries.com",
        aliases: &[],
        name: "REST Countries",
        description: "Country search and complete country listing.",
        tools: REST_COUNTRIES_TOOLS,
    },
    PrewarmedProfile {
        domain: "semanticscholar.org",
        aliases: &["api.semanticscholar.org"],
        name: "Semantic Scholar",
        description: "Scholarly paper search.",
        tools: SEMANTIC_SCHOLAR_TOOLS,
    },
    PrewarmedProfile {
        domain: "stackoverflow.com",
        aliases: &["api.stackexchange.com"],
        name: "Stack Exchange",
        description: "Stack Overflow question search.",
        tools: STACK_EXCHANGE_TOOLS,
    },
    PrewarmedProfile {
        domain: "wikidata.org",
        aliases: &["www.wikidata.org"],
        name: "Wikidata",
        description: "Wikidata entity search.",
        tools: WIKIDATA_TOOLS,
    },
];
