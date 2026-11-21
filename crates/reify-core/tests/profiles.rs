use reify_core::profiles::{self, PROFILES};
use url::Url;

#[test]
fn every_requested_domain_alias_and_wikipedia_subdomain_is_indexed() {
    for url in [
        "https://news.ycombinator.com",
        "https://hn.algolia.com",
        "https://github.com",
        "https://api.github.com",
        "https://linear.app",
        "https://dummyjson.com",
        "https://wikipedia.org",
        "https://en.wikipedia.org",
        "https://fr.m.wikipedia.org",
        "https://open-meteo.com",
        "https://api.open-meteo.com",
        "https://reddit.com",
        "https://www.reddit.com",
        "https://stripe.com",
        "https://api.stripe.com",
    ] {
        assert!(
            profiles::lookup(&Url::parse(url).unwrap(), None).is_some(),
            "{url}"
        );
    }
    assert!(profiles::lookup(&Url::parse("https://notwikipedia.org").unwrap(), None).is_none());
    assert!(profiles::lookup(&Url::parse("https://github.com.evil.test").unwrap(), None).is_none());
    for url in [
        "ftp://github.com",
        "https://user:password@github.com",
        "https://github.com:8443",
    ] {
        assert!(profiles::lookup(&Url::parse(url).unwrap(), None).is_none());
    }
    assert_eq!(PROFILES.len(), 8);
}

#[test]
fn goals_choose_the_best_matching_tools_and_unrelated_goals_fall_through() {
    let url = Url::parse("https://news.ycombinator.com").unwrap();
    let profile = profiles::lookup(&url, Some("search stories")).unwrap();
    let tools = profiles::selected_tools(profile, Some("search stories")).unwrap();
    assert_eq!(
        tools.iter().map(|tool| tool.name).collect::<Vec<_>>(),
        ["search_stories"]
    );
    assert!(profiles::lookup(&url, Some("weather forecast")).is_none());
    assert!(profiles::lookup(&url, Some("delete story")).is_none());
    let github = profiles::lookup(
        &Url::parse("https://api.github.com").unwrap(),
        Some("list issues"),
    )
    .unwrap();
    assert_eq!(
        profiles::selected_tools(github, Some("list issues")).unwrap()[0].name,
        "list_issues"
    );
}

#[test]
fn all_catalog_tools_compile_to_valid_typed_plans() {
    for profile in PROFILES {
        let tools = profiles::compile_tools(profile, None).unwrap().unwrap();
        assert_eq!(tools.len(), profile.tools.len());
        for (tool, plan) in profile.tools.iter().zip(tools) {
            assert_eq!(tool.name, plan.definition.name);
            plan.definition.validate().unwrap();
            assert!(!plan.execution.upstream_url_template.is_empty());
        }
    }
    let hn = profiles::lookup_str("https://hn.algolia.com", Some("search stories")).unwrap();
    assert_eq!(
        profiles::signature(&hn.tools[0]),
        "search_stories(q: string, tags?: string, page?: integer)"
    );
}
