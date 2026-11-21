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
];
