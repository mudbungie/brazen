+++
title = "src/native/tests.rs: stop mutating process-global HOME (pass home dir as a param)"
created = 1782204047
updated = 1782204882
claimant = "Lane3"
parent = "bl-3d74"
tags = ["cleanup", "lane3"]
+++
native/tests.rs:191-208 mutates global HOME inside the multithreaded bin-test harness. Dormant today (no sibling reads HOME; edition 2021 keeps set_var safe) but a latent unsafe env race that bites the first future XdgCredStore::new() test. FIX: factor tilde/home expansion to take the home dir as a parameter and pass a tempdir, so the test sets nothing global.