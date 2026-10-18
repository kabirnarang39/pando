//! The look of the sign-in pages: the console's brand (the Branch-P mark, bark and Fishlake
//! blue, gold only for the landing node), light and dark, in one self-contained page. Styles are
//! inline and fonts are the system's, so the strict CSP (`default-src 'none'`) holds.

use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};

use super::esc;

/// The mark and the wordmark, as in `console/src/brand.tsx`.
const LOGO: &str = concat!(
    "<svg width=28 height=28 viewBox=\"0 0 64 64\" aria-hidden=true>",
    "<path d=\"M21 13h12.5a12.5 12.5 0 0 1 0 25H25\" fill=none stroke=currentColor stroke-width=5.5 stroke-linecap=round />",
    "<path d=\"M21 10v44\" stroke=currentColor stroke-width=9 stroke-linecap=round />",
    "<circle cx=21 cy=38 r=6.2 fill=#F0B429 style=\"stroke:var(--panel)\" stroke-width=3.4 /></svg>",
    "<svg class=word viewBox=\"0 -1520 6062 1940\" aria-hidden=true><path fill=currentColor d=\"M141 390V-1080H423V-863L420 -879Q476 -997 569 -1052Q663 -1106 779 -1106Q915 -1106 1015 -1039Q1114 -971 1168 -844Q1222 -716 1222 -538Q1222 -360 1168 -234Q1113 -107 1012 -41Q912 26 774 26Q654 26 561 -29Q467 -85 409 -210L437 -212V390ZM668 -199Q777 -199 842 -291Q907 -382 907 -544Q907 -704 843 -792Q780 -881 670 -881Q601 -881 545 -844Q490 -807 457 -732Q425 -656 425 -542Q425 -371 495 -285Q565 -199 668 -199ZM1673 26Q1569 26 1482 -11Q1395 -47 1343 -119Q1290 -191 1290 -299Q1290 -415 1358 -496Q1425 -577 1552 -620Q1680 -662 1862 -662H2014L1996 -635V-697Q1996 -804 1957 -856Q1918 -909 1828 -909Q1741 -909 1696 -863Q1650 -818 1640 -734L1337 -786Q1353 -876 1414 -949Q1474 -1021 1582 -1064Q1689 -1107 1845 -1107Q1988 -1107 2087 -1062Q2186 -1016 2237 -914Q2289 -812 2289 -641V-274Q2289 -240 2298 -229Q2308 -218 2339 -218H2387V0H2245Q2129 0 2068 -59Q2007 -117 2007 -227V-278L2060 -258Q2047 -215 2021 -166Q1996 -117 1952 -73Q1908 -30 1840 -2Q1772 26 1673 26ZM1758 -178Q1829 -178 1882 -210Q1936 -242 1967 -306Q1998 -371 1998 -467V-490H1872Q1766 -490 1706 -462Q1646 -434 1622 -394Q1598 -353 1598 -315Q1598 -285 1613 -253Q1627 -221 1662 -199Q1697 -178 1758 -178ZM2526 0V-1080H2814V-831L2807 -876Q2856 -983 2946 -1044Q3036 -1106 3164 -1106Q3289 -1106 3370 -1049Q3450 -992 3489 -891Q3528 -790 3528 -658V0H3230V-639Q3230 -754 3192 -818Q3154 -881 3060 -881Q2949 -881 2887 -786Q2824 -691 2824 -495V0ZM4095 26Q3959 26 3860 -42Q3760 -110 3706 -237Q3652 -364 3652 -542Q3652 -720 3706 -846Q3761 -973 3861 -1039Q3961 -1106 4099 -1106Q4219 -1106 4313 -1051Q4406 -996 4464 -870L4434 -868V-1500H4732V0H4450V-217L4453 -202Q4399 -85 4306 -29Q4212 26 4095 26ZM4203 -199Q4273 -199 4329 -236Q4384 -273 4416 -349Q4449 -425 4449 -538Q4449 -709 4378 -795Q4308 -881 4206 -881Q4098 -881 4032 -790Q3967 -699 3967 -536Q3967 -377 4030 -288Q4093 -199 4203 -199ZM5436 26Q5273 26 5149 -46Q5024 -118 4954 -245Q4885 -373 4885 -540Q4885 -708 4954 -835Q5024 -963 5149 -1034Q5273 -1106 5436 -1106Q5600 -1106 5724 -1034Q5849 -963 5919 -835Q5989 -708 5989 -540Q5989 -372 5919 -245Q5849 -117 5724 -46Q5599 26 5436 26ZM5437 -193Q5548 -193 5613 -287Q5677 -380 5677 -540Q5677 -701 5613 -794Q5548 -887 5437 -887Q5325 -887 5261 -794Q5196 -701 5196 -540Q5196 -379 5261 -286Q5325 -193 5437 -193Z\" /></svg>",
);

const STYLE: &str = "
:root{--ground:#eef1ec;--panel:#fff;--ink:#23282d;--ink-2:#4f5a63;--line:#d6ddd3;--field:#fff;
--primary:#16324f;--primary-ink:#fff;--primary-hover:#1e4166;--link:#1d4f80;--focus:#2f6ca8;
--bad:#a3263b;--bad-wash:#f8e4e7;--lichen:#8fa89a;color-scheme:light dark}
@media (prefers-color-scheme:dark){:root{--ground:#0b141e;--panel:#101b27;--ink:#e6ecf2;--ink-2:#a0b0c1;
--line:#29394b;--field:#0b141e;--primary:#8cb8e6;--primary-ink:#0b141e;--primary-hover:#a9cbee;--link:#8cb8e6;
--focus:#8cb8e6;--bad:#f08a97;--bad-wash:#2c1419}}
*{box-sizing:border-box}
html{background:var(--ground);color:var(--ink);font:16px/1.5 ui-sans-serif,system-ui,-apple-system,\"Segoe UI\",Roboto,\"Helvetica Neue\",Arial,sans-serif;-webkit-font-smoothing:antialiased}
body{margin:0;min-height:100vh;display:grid;align-content:start;justify-items:center;padding:12vh 16px 48px}
main{width:100%;max-width:26rem;padding:32px;border:1px solid var(--line);border-radius:16px;background:var(--panel)}
.brand{display:flex;align-items:center;gap:8px;margin-bottom:28px;color:var(--ink)}
.brand .word{height:17px;width:auto}
h1{margin:0 0 12px;font-size:1.5rem;line-height:1.25;letter-spacing:-.01em}
p{margin:0 0 16px}p:last-child{margin-bottom:0}
a{color:var(--link);text-underline-offset:2px}
strong{font-weight:650;overflow-wrap:anywhere}
code{font:.875em ui-monospace,\"SF Mono\",Menlo,Consolas,monospace;padding:1px 5px;border-radius:5px;background:var(--ground)}
form{display:grid;gap:16px;margin:0}form p{margin:0}form+p,div+p.muted{margin-top:16px}
label{display:grid;gap:6px;font-size:.875rem;font-weight:600}
input{font:inherit;font-weight:400;height:44px;padding:0 12px;border:1px solid var(--line);border-radius:8px;background:var(--field);color:var(--ink)}
input::placeholder{color:var(--ink-2);font-weight:400}
button,.button{display:flex;align-items:center;justify-content:center;width:100%;min-height:44px;padding:0 16px;
border:1px solid var(--primary);border-radius:8px;background:var(--primary);color:var(--primary-ink);font:inherit;font-weight:600;text-decoration:none;cursor:pointer}
button:hover,.button:hover{background:var(--primary-hover);border-color:var(--primary-hover)}
.quiet,.quiet:hover{background:transparent;border-color:var(--line);color:var(--ink)}
.quiet:hover{background:var(--ground)}
:focus-visible{outline:2px solid var(--focus);outline-offset:2px}
input:focus-visible{outline-offset:0;border-color:var(--focus)}
.providers{display:grid;gap:8px;margin-bottom:16px}
.or{display:flex;align-items:center;gap:12px;color:var(--ink-2);font-size:.875rem}
.or::before,.or::after{content:\"\";flex:1;border-top:1px solid var(--line)}
.check{display:flex;gap:10px;align-items:flex-start;font-weight:400;font-size:1rem}
.check input{width:20px;height:20px;margin:2px 0 0;flex:none;accent-color:var(--primary)}
.actions{display:flex;gap:8px}
.muted{color:var(--ink-2);font-size:.875rem}
[role=alert]{padding:10px 12px;border-radius:8px;background:var(--bad-wash);color:var(--bad)}
.code{font:600 1.25rem ui-monospace,\"SF Mono\",Menlo,Consolas,monospace;letter-spacing:.08em}
@media (max-width:480px){body{padding:24px 16px}main{padding:24px 20px;border-radius:12px}}
";

/// A sign-in page; `body` is HTML whose dynamic parts are already escaped.
pub(super) fn page(status: StatusCode, title: &str, body: &str) -> Response {
    let html = format!(
        "<!doctype html><html lang=en><meta charset=utf-8><meta name=viewport content=\"width=device-width,initial-scale=1\">         <meta name=color-scheme content=\"light dark\"><title>{t} – Pando</title><style>{STYLE}</style>         <main><div class=brand role=img aria-label=Pando>{LOGO}</div><h1>{t}</h1>{body}</main>",
        t = esc(title)
    );
    (status, [(header::CONTENT_TYPE, "text/html; charset=utf-8")], html).into_response()
}
