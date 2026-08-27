/*
 * rescriptum's branding for iPXE.
 *
 * Copied over src/config/local/branding.h in a pinned iPXE checkout. See README.md
 * beside this file for what is built and why we build it at all.
 *
 * Two rules, and both come from upstream's own comment in src/config/branding.h:
 *
 *   - PRODUCT_SHORT_NAME should either be a substring of PRODUCT_NAME or stay "iPXE",
 *     "to minimise end-user confusion". It stays "iPXE": what appears in a BIOS boot
 *     selection menu should say what the thing actually is.
 *
 *   - The error and command-help URIs are **deliberately left alone**. They point at
 *     ipxe.org's database, which turns a 32-bit error code into a sentence, names the
 *     source file that produced it, and links the line of code. Redirecting them at us
 *     would replace a working diagnostic service with nothing — the operator staring at
 *     a hex code at 3am is the person that database exists for.
 *
 * Keeping the iPXE attribution is also the right way to use somebody's GPLv2 work.
 */

#ifndef CONFIG_LOCAL_BRANDING_H
#define CONFIG_LOCAL_BRANDING_H

#undef PRODUCT_NAME
#undef PRODUCT_SHORT_NAME
#undef PRODUCT_URI
#undef PRODUCT_TAG_LINE

/* Shown before any iPXE branding, which is the first line a machine puts on screen. */
#define PRODUCT_NAME "rescriptum boot"
#define PRODUCT_SHORT_NAME "iPXE"
#define PRODUCT_URI "https://github.com/z29k/rescriptum"
#define PRODUCT_TAG_LINE "Every machine its own install"

#endif /* CONFIG_LOCAL_BRANDING_H */
