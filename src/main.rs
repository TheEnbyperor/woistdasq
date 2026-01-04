#[macro_use] extern crate rocket;
#[macro_use] extern crate log;
use rocket::serde::Deserialize;
use std::str::FromStr;
use chrono::prelude::*;

#[derive(Deserialize)]
#[serde(crate = "rocket::serde")]
struct Config {
    ical_url: String,
    travelynx_token: String,
}

#[derive(Debug, Deserialize)]
#[serde(crate = "rocket::serde")]
struct TravelynxRaw {
    #[serde(rename = "checkedIn")]
    checked_in: bool,
    #[serde(rename = "fromStation")]
    from_station: TravelynxStation,
    #[serde(rename = "toStation")]
    to_station: TravelynxStation,
}

#[derive(Debug, Deserialize)]
#[serde(crate = "rocket::serde")]
struct TravelynxStation {
    #[serde(rename = "realTime")]
    real_time: i64,
    #[serde(rename = "scheduledTime")]
    scheduled_time: i64,
}

#[derive(Debug)]
struct Travelynx {
    checked_in: bool,
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

async fn get_travelyx(client: &reqwest::Client, config: &Config) -> Result<Travelynx, rocket::http::Status> {
    let r = client.get(format!("https://travelynx.de/api/v1/status/{}", config.travelynx_token))
        .send().await.map_err(|_| rocket::http::Status::InternalServerError)?
        .json::<TravelynxRaw>().await.map_err(|_| rocket::http::Status::InternalServerError)?;

    let start = std::cmp::min(r.from_station.scheduled_time, r.from_station.real_time);
    let end = std::cmp::min(r.to_station.scheduled_time, r.to_station.real_time);

    Ok(Travelynx {
        checked_in: r.checked_in,
        start: Utc.timestamp_opt(start, 0).earliest().ok_or(rocket::http::Status::InternalServerError)?,
        end: Utc.timestamp_opt(end, 0).latest().ok_or(rocket::http::Status::InternalServerError)?,
    })
}

#[derive(Debug)]
enum Status<'a> {
    Travelynx,
    Flight(&'a str),
}

fn nearest_event<'a>(flights: &'a[Flight], travelynx: &Travelynx) -> Status<'a> {
    let now = Utc::now();

    if (travelynx.start < now && now < travelynx.end) || travelynx.checked_in {
        return Status::Travelynx;
    }

    for flight in flights {
        if flight.start < now && now < flight.end {
            return Status::Flight(&flight.flighty_id);
        }
    }

    let first_flight = match flights.first() {
        Some(f) => f,
        None => return Status::Travelynx,
    };

    let travelynx_diff = travelynx.start - now;
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
        Status::Travelynx
    }
}

#[get("/")]
async fn index(config: &rocket::State<Config>) -> Result<rocket::response::Redirect, rocket::http::Status> {
    let client = reqwest::Client::new();

    let flights = get_flights(&client, &config).await?;
    let travelynx = get_travelyx(&client, &config).await?;

    Ok(match nearest_event(&flights, &travelynx) {
        Status::Travelynx =>  rocket::response::Redirect::temporary("https://travelynx.de/p/q"),
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

#[launch]
fn rocket() -> _ {
    pretty_env_logger::init();

    rocket::build()
        .attach(rocket::fairing::AdHoc::config::<Config>())
        .mount("/", routes![
            index,
            flighty_status,
            travelynx_status,
        ])
}