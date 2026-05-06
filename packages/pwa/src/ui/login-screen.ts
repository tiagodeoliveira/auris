//! Pre-auth landing screen. Shown when there's no active Auth0
//! session; clicking the button redirects to Auth0's hosted login.
//! Once the round-trip completes, `main.ts` re-runs and skips this
//! component entirely — there's no in-app post-login transition to
//! manage.

import type { AuthBundle } from "../auth";

export function mountLoginScreen(parent: HTMLElement, auth: AuthBundle): void {
  parent.innerHTML = "";

  const wrap = document.createElement("section");
  wrap.className = "login-screen";

  const title = document.createElement("h1");
  title.className = "login-screen-title";
  title.textContent = "Meeting Companion";

  const subtitle = document.createElement("p");
  subtitle.className = "login-screen-subtitle";
  subtitle.textContent = "Sign in to capture your meetings.";

  // Button is a generic "Sign in" rather than picking an IdP — Auth0
  // Universal Login lets the user choose their identity provider on
  // the hosted page once they land there, and we don't want to bake
  // one provider into the CTA when the tenant might enable several.
  const btn = document.createElement("button");
  btn.className = "btn-primary login-screen-button";
  btn.textContent = "Sign in";
  btn.addEventListener("click", () => {
    btn.disabled = true;
    btn.textContent = "Redirecting…";
    auth.loginWithRedirect().catch((e) => {
      btn.disabled = false;
      btn.textContent = "Sign in";
      console.warn("[login] redirect failed", e);
    });
  });

  wrap.append(title, subtitle, btn);
  parent.appendChild(wrap);
}
