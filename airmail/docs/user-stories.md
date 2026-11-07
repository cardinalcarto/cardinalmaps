# Geocoder user stories

Status: agreed. The query design is in [query-design.md](query-design.md).

"Viewport" is what the map shows when the user starts typing. The geocoder receives it as a bounding box, along with the user's location. "App" queries are sent as the user types, with the last word possibly unfinished.

### A. Getting to an address

| # | Intent | Viewport | Types | Should get |
|---|---|---|---|---|
| A1 | Navigate home | Seattle, neighbourhood zoom | `400 broad st seattle` | That address first |
| A2 | Same, before finishing | Seattle | `400 broad st` | The local address above other cities' |
| A3 | Address in another city | Seattle | `123 main street spokane` | The Spokane address |
| A4 | Address with no city, far from the map | Spokane | `1530 post alley` | The only match, in Seattle, however far |
| A5 | Common address, no city | Seattle | `123 main st` | Nearest first, others following |
| A6 | Apartment | Seattle | `400 broad st apt 4` | The building, even if the unit isn't mapped |
| A7 | Foreign format | Seattle | `clayallee 170 14191 berlin` | That Berlin address, never a category search |
| A8 | Find a street, not a number | Seattle | `pike street` | The nearby street |
| A9 | Postcode | Seattle | `98107` | The postcode area |
| A10 | Address containing a place word | Seattle | `100 market street` | The address, not markets |

### B. A particular place

| # | Intent | Viewport | Types | Should get |
|---|---|---|---|---|
| B1 | Explore a famous place | Seattle, city zoom | `eiffel tower` | The one in Paris, 8,000 km away, above any local tower |
| B2 | Local landmark | Seattle | `space needle` | It, immediately |
| B3 | Local landmark from elsewhere | Spokane | `space needle` | Still it |
| B4 | Small business by full name | Anywhere in WA | `charlie's queer books` | It, with or without the apostrophe |
| B5 | Regional park from afar | Spokane | `golden gardens` | Golden Gardens Park |
| B6 | Natural feature | Seattle | `mount rainier` | The mountain, not Mount Baker the neighbourhood |
| B7 | Colloquial name | Seattle | `sea tac airport` | Seattle-Tacoma International Airport |
| B8 | Name in another script | Seattle | `東京タワー` | Tokyo Tower |
| B9 | Misspelled name | Seattle | `space neeedle` | Space Needle |
| B10 | Name with a word that's also a kind | Seattle | `museum of glass` | Museum of Glass, in Tacoma, not nearby museums |
| B11 | Name that contains an area | Seattle | `seattle art museum` | It, not museums in Seattle |
| B12 | A lesser place with a famous namesake | Paris suburb | `eiffel tower` | The real one, or a local one only if it's truly an exact name and nearby. |

### C. A chain

| # | Intent | Viewport | Types | Should get |
|---|---|---|---|---|
| C1 | Nearest of a chain | Fremont | `starbucks` | Mostly Starbucks by distance, though a cafe very close by can rank above one a few km away |
| C2 | Chain in a named area | Downtown | `starbucks ballard` | Those in and near Ballard |
| C3 | Chain with none nearby | Rural eastern WA | `starbucks` | Nearby cafes, with the nearest Starbucks always kept in the results, even at 60 km |
| C4 | Chain whose name holds a kind | Seattle | `pizza hut` | Pizza Hut, not pizza |
| C5 | Chain that's also an ordinary word | Seattle | `target` | Target stores |
| C6 | Chain and its relatives | Seattle | `value village` | Value Village first, then perhaps other second-hand shops |

### D. A kind of place

| # | Intent | Viewport | Types | Should get |
|---|---|---|---|---|
| D1 | Any nearby place of a kind | Fremont | `thrift store` | Goodwill and others by distance, whether tagged charity or second-hand, however they're named |
| D2 | Kind in a named neighbourhood | Downtown | `thrift store ballard` | Ballard's, including the Goodwill just over the data's boundary |
| D3 | Kind in another city | Seattle | `pizza spokane` | Pizza in Spokane |
| D4 | Kind in a place where there's none | Downtown | `museum fremont` | The nearest ones, not an empty screen |
| D5 | Kind in the local language | Seattle, Spanish phone | `farmacia` | Pharmacies |
| D6 | Cuisine | Capitol Hill | `thai food` | Thai restaurants nearby |
| D7 | Diet and cuisine | Seattle | `vegan pizza` | Vegan pizza first, other pizza after |
| D8 | Kind, zoomed out | All of WA | `hospital` | Major hospitals across what's visible, not just the middle |
| D9 | Word that names a place somewhere | Seattle | `bar` | Bars, not Bar in Montenegro. With `prefix` still typing, a town may appear |
| D10 | Kind with a place-named shop elsewhere | Seattle | `bookstore` | Nearby bookshops, not a place named "Bookstore" 225 km off |
| D11 | Mapped furniture without a name | Green Lake | `drinking fountain`, `bench`, `ev charger` | The nearest ones (needs the unnamed POI work) |

### E. An area

| # | Intent | Viewport | Types | Should get |
|---|---|---|---|---|
| E1 | Move the map to a city | Anywhere | `seattle` | The city |
| E2 | A neighbourhood | Seattle | `ballard` | Seattle's Ballard |
| E3 | A city from elsewhere | Seattle | `new york` | New York, not a local place with those words |
| E4 | Far-off city | Seattle | `london`, `paris` | The famous ones |
| E5 | Ambiguous, near the lesser one | Downtown Vancouver, WA | `vancouver` | Vancouver, WA |
| E6 | Ambiguous, near neither | Seattle | `vancouver` | Vancouver, BC |
| E7 | Qualified | Seattle | `vancouver wa` | Vancouver, WA |
| E8 | Many namesakes | Seattle | `springfield` | The best known |
| E9 | Small town near a famous one | Seattle | `leavenworth` | Leavenworth, WA, over Kansas |
| E10 | County, region, country | Seattle | `king county`, `oregon`, `japan` | That area |
| E11 | A name in another language | Seattle | `europa`, `wien` | Europe, Vienna |

### F. Exploring

| # | Intent | Viewport | Types | Should get |
|---|---|---|---|---|
| F1 | Look around a far city | Seattle | `coffee paris` | Cafes in Paris |
| F2 | Look around where the map is | Tokyo, panned there | `ramen` | Ramen near the map, not near the user |
| F3 | Famous things, zoomed out | World | `museum` | The best-known museums |
| F4 | A place, zoomed out | World | `tower` | Famous towers, in no special place |
| F5 | Exact place in a far area | Seattle | `louvre paris` | The Louvre |

### G. While typing

Results update on each keystroke, and each state should look reasonable.

| # | Intent | Viewport | Types | Should get |
|---|---|---|---|---|
| G1 | First letters | Seattle | `s`, `se` | Well-known and nearby places, quickly |
| G2 | Partial name | Seattle | `pike plac` | Pike Place Market |
| G3 | Partial area | Anywhere | `broo` | Brooklyn among results |
| G4 | Partial word that isn't an area | Seattle | `daly ci` | Daly City |
| G5 | Kind, area unfinished | Downtown | `bookstore frem` | Fremont's bookshops once it's clear, nearby ones before |
| G6 | Area first, kind unfinished | Seattle | `seattle cof` | Coffee in Seattle |
| G7 | Finished word | Seattle | `bakery ` (trailing space) | Treated as a finished word |
| G8 | Submitted search | Seattle | Presses search | Treated as finished, not as still being typed |

### H. Language and failure

| # | Intent | Viewport | Types | Should get |
|---|---|---|---|---|
| H1 | Other language's word, English phone | Seattle | `farmacia` | Places named that first. Pharmacies only if another phone language has a model |
| H2 | No model for the language | Seattle, German phone | `apotheke` | Name matches, without error |
| H3 | Nonsense | Seattle | `asdfgh` | Nothing, or typo matches clearly weaker than usual |
| H4 | Tagger misreads a word | Seattle | `bookshop fremont` | Shops in Fremont even though "bookshop" reads as a neighbourhood |
| H5 | Misspelled area | Seattle | `bookstore freemont` | Good if cheap. Not worth special handling |

### I. Near the user, not the map

| # | Intent | Viewport | Types | Should get |
|---|---|---|---|---|
| I1 | Need something now, map elsewhere | Paris, user in Seattle | `gas station near me` | Near the user |
