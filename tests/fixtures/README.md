Integration tests read the real `Money.xlsx` workbook, wherever `MM_WORKBOOK`
says it is.

It is not committed -- it is personal financial data, and neither is its path,
which is why there is no default. Tests that need it are skipped when
`MM_WORKBOOK` is unset or names a file that is not there, so `cargo test`
passes on a clean checkout.
