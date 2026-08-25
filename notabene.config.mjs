// The documentation site: `docs/` rendered by notabene, published to GitHub Pages by
// .github/workflows/docs.yml (`build --public` → z29k.github.io/rescriptum).
//
// This file is the only thing the docs toolchain needs; the Rust binary knows nothing
// about it. `npm run docs` opens the review server, `npm run docs:build` produces the
// static artifact CI uploads.
export default {
  siteName: "rescriptum",
  // Plain string: `tagline` is one of the few config strings notabene does NOT accept a
  // per-locale map for — a map stringifies to "[object Object]" in the topbar and llms.txt.
  tagline: "an answer written for this machine",
  locale: "en",
  // Plain CommonMark: these pages are read on GitHub as often as on the site, and an
  // MDX component that renders to nothing in a diff would be a bad trade.
  format: "commonmark",

  // Two audiences, two spaces. Running the server and writing answers has nothing to
  // do with hacking on it, and mixing the two makes both harder to read.
  roots: [
    {
      key: "guide",
      label: "Guide",
      path: "docs/guide",
      description: {
        en: "Install rescriptum, serve a first answer, write answers that compose, and run it in production.",
        fr: "Installer rescriptum, servir une première réponse, écrire des réponses qui se composent, et le faire tourner en production.",
      },
    },
    {
      key: "development",
      label: { en: "Development", fr: "Développement" },
      path: "docs/development",
      description: {
        en: "How the server is built and why: the constraints, the request path, the internals, the tests, the release.",
        fr: "Comment le serveur est construit, et pourquoi : les contraintes, le trajet d'une requête, les internes, les tests, la release.",
      },
    },
  ],

  store: "docs/.notabene",

  // Docs are merged into `develop`; `main` only ever receives release commits.
  editPattern: "https://github.com/z29k/rescriptum/edit/develop/{path}",

  // A README-like welcome, rendered above the two space cards on "/". Deliberately
  // outside both spaces so it is not also a doc page.
  home: { en: "docs/home.md", fr: "docs/home.fr.md" },

  // Bilingual docs, suffix strategy: the English files keep their paths and URLs, the
  // French ones are `*.fr.md` siblings. That matters for a doc that already exists —
  // nothing moves, and the comment threads on the English pages survive.
  i18n: { locales: ["en", "fr"], defaultLocale: "en", strategy: "suffix" },

  // Identity: the sealed rescript on a floppy disk — a written answer, delivered by a
  // machine. Same image as topbar logo, favicon and social card.
  branding: {
    logo: "assets/rescriptum-logo.jpg",
    favicon: "assets/rescriptum-logo.jpg",
    socialImage: "assets/rescriptum-logo.jpg",
  },

  nav: {
    header: [
      { label: "GitHub", href: "https://github.com/z29k/rescriptum", icon: "github", iconOnly: true },
    ],
    sidebar: {
      title: { en: "Resources", fr: "Ressources" },
      links: [
        { label: "GitHub", href: "https://github.com/z29k/rescriptum", icon: "github" },
        {
          label: { en: "Releases", fr: "Versions" },
          href: "https://github.com/z29k/rescriptum/releases",
          icon: "download",
        },
        {
          label: { en: "Proxmox: Automated Installation", fr: "Proxmox : installation automatisée" },
          href: "https://pve.proxmox.com/wiki/Automated_Installation",
          icon: "book",
        },
      ],
    },
    footer: {
      links: [
        { label: { en: "Issues", fr: "Tickets" }, href: "https://github.com/z29k/rescriptum/issues" },
        {
          label: { en: "MIT licence", fr: "Licence MIT" },
          href: "https://github.com/z29k/rescriptum/blob/main/LICENSE",
        },
      ],
      text: "© 2026 z29k",
      poweredBy: true,
    },
  },

  // The agent proposes, a human validates each edit against its real git diff at
  // /review. Docs that describe root passwords and boot-time configuration are worth
  // reading before they ship. Set "auto" to let the agent resolve comments itself.
  review: "approve",

  publish: { site: "https://z29k.github.io", base: "/rescriptum" },
};
