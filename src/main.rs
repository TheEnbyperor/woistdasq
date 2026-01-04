#[macro_use] extern crate rocket;
#[macro_use] extern crate log;
use rocket::serde::Deserialize;
use std::str::FromStr;
use chrono::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
struct Config {
    ical_url: String,
    traewelling_username: String,
}

// #[derive(Debug, Deserialize)]
// #[serde(crate = "rocket::serde")]
// struct TravelynxRaw {
//     #[serde(rename = "checkedIn")]
//     checked_in: bool,
//     #[serde(rename = "fromStation")]
//     from_station: TravelynxStation,
//     #[serde(rename = "toStation")]
//     to_station: TravelynxStation,
// }
//
// #[derive(Debug, Deserialize)]
// #[serde(crate = "rocket::serde")]
// struct TravelynxStation {
//     #[serde(rename = "realTime")]
//     real_time: i64,
//     #[serde(rename = "scheduledTime")]
//     scheduled_time: i64,
// }

#[derive(Debug, Deserialize)]
#[serde(crate = "rocket::serde")]
struct TraewellingRaw {
    data: Vec<TraewellingTrip>,
}

#[derive(Debug, Deserialize)]
#[serde(crate = "rocket::serde")]
struct TraewellingTrip {
    id: u64,
    train: TraewellingTrain,
}

#[derive(Debug, Deserialize)]
#[serde(crate = "rocket::serde")]
struct TraewellingTrain {
    origin: TraewellingStation,
    destination: TraewellingStation,
}

#[derive(Debug, Deserialize)]
#[serde(crate = "rocket::serde")]
struct TraewellingStation {
    arrival: DateTime<Utc>,
    departure: DateTime<Utc>,
}

#[derive(Debug)]
struct Traewelling {
    id: u64,
    start: DateTime<Utc>,
    end: DateTime<Utc>,
}

#[derive(Debug)]
struct Flight {
    start: DateTime<Utc>,
    end: DateTime<Utc>,
    flighty_id: String,
}

fn parse_event_to_flight(event: icalendar::Event) -> Option<Flight> {
    use icalendar::Component;

    lazy_static::lazy_static! {
        static ref RE: regex::Regex = regex::Regex::new("flighty://flight/([\\w-]+)").unwrap();
    }

    let start = match event.get_start()? {
        icalendar::DatePerhapsTime::DateTime(dt) => dt.try_into_utc()?,
        _ => return None,
    };
    let end = match event.get_end()? {
        icalendar::DatePerhapsTime::DateTime(dt) => dt.try_into_utc()?,
        _ => return None,
    };
    let desc = match RE.captures(event.get_description()?) {
        Some(caps) => caps.get(1)?.as_str(),
        None => return None,
    };

    Some(Flight {
        start,
        end,
        flighty_id: desc.to_string(),
    })
}

async fn get_flights(client: &reqwest::Client, config: &Config) -> Result<Vec<Flight>, rocket::http::Status> {
    let c = client.get(&config.ical_url)
        .send().await.map_err(|_| rocket::http::Status::InternalServerError)?
        .text().await.map_err(|_| rocket::http::Status::InternalServerError)?;

    let cal = icalendar::Calendar::from_str(&c).map_err(|e| {
        warn!("{}", e);
        rocket::http::Status::InternalServerError
    })?;

    let mut flights = cal.components.into_iter().filter_map(|c| {
        if let icalendar::CalendarComponent::Event(event) = c {
            match event.get_status() {
                None | Some(icalendar::EventStatus::Confirmed) => Some(event),
                _ => None
            }
        } else {
            None
        }
    }).filter_map(parse_event_to_flight).collect::<Vec<_>>();

    let now = Utc::now();
    flights.sort_unstable_by_key(|f| {
        let diff = f.start - now;
        let diff = if diff < chrono::Duration::zero() {
            diff * -1
        } else {
            diff
        };
        diff
    });

    Ok(flights)
}

// async fn get_travelyx(client: &reqwest::Client, config: &Config) -> Result<Travelynx, rocket::http::Status> {
//     let r = client.get(format!("https://travelynx.de/api/v1/status/{}", config.travelynx_token))
//         .send().await.map_err(|_| rocket::http::Status::InternalServerError)?
//         .json::<TravelynxRaw>().await.map_err(|_| rocket::http::Status::InternalServerError)?;
//
//     let start = std::cmp::min(r.from_station.scheduled_time, r.from_station.real_time);
//     let end = std::cmp::min(r.to_station.scheduled_time, r.to_station.real_time);
//
//     Ok(Travelynx {
//         checked_in: r.checked_in,
//         start: Utc.timestamp_opt(start, 0).earliest().ok_or(rocket::http::Status::InternalServerError)?,
//         end: Utc.timestamp_opt(end, 0).latest().ok_or(rocket::http::Status::InternalServerError)?,
//     })
// }

async fn get_traewelling(client: &reqwest::Client, config: &Config) -> Result<Traewelling, rocket::http::Status> {
    let r = client.get(format!(" https://traewelling.de/api/v1/user/{}/statuses?limit=1", config.traewelling_username))
        .send().await.map_err(|_| rocket::http::Status::InternalServerError)?
        .json::<TraewellingRaw>().await.map_err(|_| rocket::http::Status::InternalServerError)?;

    if r.data.is_empty() {
        return Ok(Traewelling {
            id: 0,
            start: DateTime::<Utc>::MAX_UTC,
            end: DateTime::<Utc>::MIN_UTC,
        })
    }

    Ok(Traewelling {
        id: r.data[0].id,
        start: r.data[0].train.origin.arrival,
        end: r.data[0].train.destination.departure,
    })
}

#[derive(Debug)]
enum Status<'a> {
    Traewelling(u64),
    Flight(&'a str),
}

fn nearest_event<'a>(flights: &'a[Flight], traewelling: &Traewelling) -> Status<'a> {
    let now = Utc::now();

    if traewelling.start < now && now < traewelling.end {
        return Status::Traewelling(traewelling.id);
    }

    for flight in flights {
        if flight.start < now && now < flight.end {
            return Status::Flight(&flight.flighty_id);
        }
    }

    let first_flight = match flights.first() {
        Some(f) => f,
        None => return Status::Traewelling(traewelling.id),
    };

    let travelynx_diff = traewelling.start - now;
    let flight_diff = first_flight.start - now;
    let travelynx_diff = if travelynx_diff < chrono::Duration::zero() {
        travelynx_diff * -1
    } else {
        travelynx_diff
    };
    let flight_diff = if flight_diff < chrono::Duration::zero() {
        flight_diff * -1
    } else {
        flight_diff
    };

    if flight_diff < travelynx_diff {
        Status::Flight(&first_flight.flighty_id)
    } else {
        Status::Traewelling(traewelling.id)
    }
}

#[get("/")]
async fn index(config: &rocket::State<Config>) -> Result<rocket::response::Redirect, rocket::http::Status> {
    let client = reqwest::Client::new();

    let flights = get_flights(&client, &config).await?;
    let traewelling = get_traewelling(&client, &config).await?;

    Ok(match nearest_event(&flights, &traewelling) {
        Status::Traewelling(id) if id == 0 => rocket::response::Redirect::temporary(format!("https://traewelling.de/@{}", config.traewelling_username)),
        Status::Traewelling(id) => rocket::response::Redirect::temporary(format!("https://traewelling.de/status/{}", id)),
        Status::Flight(id) => rocket::response::Redirect::temporary(format!("https://live.flighty.app/{}", id)),
    })
}

#[get("/<id>")]
fn travelynx_status(id: &str) -> rocket::response::Redirect {
    rocket::response::Redirect::temporary(format!("https://travelynx.de/status/q/{}", id))
}

#[get("/f/<id>")]
fn flighty_status(id: &str) -> rocket::response::Redirect {
    rocket::response::Redirect::temporary(format!("https://live.flighty.app/{}", id))
}

#[get("/t/<id>")]
fn traewelling_status(id: &str) -> rocket::response::Redirect {
    rocket::response::Redirect::temporary(format!("https://traewelling.de/status/{}", id))
}

#[launch]
fn rocket() -> _ {
    pretty_env_logger::init();

    rocket::build()
        .attach(rocket::fairing::AdHoc::config::<Config>())
        .mount("/", routes![
            index,
            flighty_status,
            travelynx_status,
            traewelling_status,
        ])
}