(term) => {
    const forbidden = /password|email|tel|payment|card|delete|checkout/i;
    const discovery = /search|query|find|filter|(?:^|\W)q(?:$|\W)/i;
    const metadata = (element) => [
        ...["type", "name", "id", "placeholder", "aria-label", "autocomplete",
            "title", "action", "formaction"].map(name => element.getAttribute(name) || ""),
        ...Array.from(element.labels || [], label => label.textContent || ""),
        element.matches("button, input[type=submit], input[type=button]")
            ? (element.textContent || "") + " " + (element.value || "") : ""
    ].join(" ");
    const visible = element => !element.matches(":disabled") && !element.readOnly &&
        !element.closest("[hidden], [inert], [aria-hidden=true]") &&
        getComputedStyle(element).visibility !== "hidden" && element.getClientRects().length > 0;
    const safe = element => {
        if (!visible(element) || forbidden.test(metadata(element))) return false;
        if (element.hasAttribute("formmethod") && element.formMethod.toLowerCase() !== "get") return false;
        if (element.hasAttribute("formtarget") && element.formTarget !== "_self") return false;
        const form = element.form;
        if (!form) return true;
        if (forbidden.test(metadata(form) + " " + form.textContent)) return false;
        if (Array.from(form.elements).some(control => forbidden.test(metadata(control)))) return false;
        const destination = new URL(form.action, location.href);
        return ["http:", "https:"].includes(destination.protocol) &&
            !destination.username && !destination.password &&
            form.method.toLowerCase() === "get" &&
            (!form.target || form.target === "_self");
    };
    const inputs = Array.from(document.querySelectorAll("input"));
    const input = inputs.find(element =>
        (element.type === "search" || (element.type === "text" && discovery.test(metadata(element)))) &&
        safe(element));
    if (input) {
        const form = input.form;
        const destination = form ? form.action : null;
        const stillSafe = () => input.form === form && safe(input) &&
            (!form || form.action === destination);
        const result = { acted: true, destination };
        input.focus();
        if (!stillSafe()) return result;
        // Use the native setter so controlled inputs observe the subsequent event.
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, term);
        input.dispatchEvent(new Event("input", { bubbles: true, composed: true }));
        if (!stillSafe()) return result;
        input.dispatchEvent(new Event("change", { bubbles: true, composed: true }));
        if (stillSafe()) {
            const options = { key: "Enter", code: "Enter", keyCode: 13, which: 13, bubbles: true, cancelable: true };
            const enter = input.dispatchEvent(new KeyboardEvent("keydown", options));
            if (stillSafe()) input.dispatchEvent(new KeyboardEvent("keyup", options));
            // Synthetic Enter has no browser default action. Submit only a checked
            // GET search form, after returning the destination to the capture loop.
            if (enter && form && stillSafe()) {
                setTimeout(() => {
                    if (stillSafe()) HTMLFormElement.prototype.requestSubmit.call(form);
                }, 0);
            }
        }
        return result;
    }
    const button = Array.from(document.querySelectorAll("button, input[type=button], input[type=submit]"))
        .find(element => ["button", "submit"].includes(element.type) &&
            discovery.test(metadata(element)) && safe(element) &&
            (!element.form || !element.getAttribute("formaction")));
    if (button) {
        const destination = button.form ? button.form.action : null;
        setTimeout(() => { if (safe(button)) button.click(); }, 0);
        return { acted: true, destination };
    }
    return { acted: false };
}
