require ["copy", "envelope", "editheader", "spamtest", "relational", "comparator-i;ascii-numeric"];
# k2-forward
if envelope :is "from" "" {
keep;
stop;
}
if address :is "from" ["k2@brand.test", "system@notify.brand.test"] {
keep;
stop;
}
if header :contains "X-K2-Forwarded" "mail.brand.test" {
keep;
stop;
}
if anyof (spamtest :value "ge" :comparator "i;ascii-numeric" "5", header :matches "X-Spam-Score" "spam*") {
keep;
stop;
}
if anyof (header :contains "Authentication-Results" "dmarc=fail", header :matches "Authentication-Results" ["mail.brand.test;*dmarc=permerror*", "mail.brand.test;*dmarc=temperror*", "mail.brand.test;*dmarc=none*header.from= *", "mail.brand.test;*dmarc=none*header.from="], allof (header :matches "Authentication-Results" "mail.brand.test;*", not header :contains "Authentication-Results" "dmarc=")) {
keep;
stop;
}
addheader "X-K2-Forwarded" "mail.brand.test";
redirect :copy "me@outside.test";
