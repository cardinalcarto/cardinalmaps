Address formats from OpenCage's [address-formatting](https://github.com/OpenCageData/address-formatting)
templates (MIT, see `LICENSE.txt`), at the commit in `VERSION`: `conf/components.yaml`,
`conf/countries/worldwide.yaml`, `conf/state_codes.yaml` and `conf/county_codes.yaml`, unchanged.
`airmail::address_format` processes them, for the query tagger's training data.

To update, copy those files from a newer checkout and run the formatter's tests against its test
cases: `ADDRESS_FORMATTING=path/to/address-formatting cargo test -p airmail address_format`.
