// https://api.hanatour.com/svc/comMpgApiCategory/getResListApi?_siteId=hanatour
#[derive(serde::Serialize, Debug)]
struct GetReservationListRequest {
    inpPathCd: String,   // "DCP"
    siteCd: String,      // "C00002S001"
    chnlCd: String,      // "DPC"
    resPathCd: String,   // "DCP"
    ptnCd: String,       // ""
    startDate: String,   // YYYYMMDD
    endDate: String,     // YYYYMMDD
    resStatus: String,   // "Y"
    sort: String,        // "res"
    resAttrCd: String,   // "A"
    webtourFlag: String, // "false"
}

#[derive(serde::Deserialize, Debug)]
struct GetReservationListResponse {
    getResListConfig: GetResListConfig,
}

#[derive(serde::Deserialize, Debug)]
struct GetResListConfig {
    resListInfo: Vec<ResListInfo>,
}

#[derive(serde::Deserialize, Debug)]
struct ResListInfo {
    resComCd: String,
    airFarCombResNum: Option<String>,
    airFarCombResSeq: String,
    resCd: String,
    resId: String,
    unfyResCd: Option<String>,
    gds1pnrNum: String,
    seatStatCd: String,
    isueRstatCd: String,
    resDttm: String,
    cnclDttm: Option<String>,
    totAmt: String,
    depDt: String,
    arrDt: String,
    hmcmgDt: String,
    custPayTlDt: String,
    custPayTlHm: String,
    itnrTypeCd: String,
    depCityCd: String,
    depCityNm: String,
    arrCityCd: String,
    arrCityNm: String,
    isueAirlCd: String,
    isueAirlNm: String,
    adtCnt: String,
    chdCnt: String,
    infCnt: String,
    totPaxCnt: String,
    totPaxCnlCnt: String,
    gdsDvCd: String,
    resCnclStatCd: String,
    airResCretStatCd: String,
    airSiteCd: String,
    airSiteNm: String,
}

pub(super) async fn crawl(
    config: super::Config,
    user_id: UserId,
    db: &SqlitePool,
) -> anyhow::Result<usize> {
    flatten_error(
        tokio::task::spawn_blocking(move || -> anyhow::Result<_> {
            use headless_chrome::protocol::cdp::types::Event;
            let browser = open_browser()?;

            let tab = browser.new_tab()?;
            info!("Open Bustago login page");
            tab.navigate_to("https://accounts.hanatour.com/?redirectUri=https%3A%2F%2Fm.hanatour.com%2Fcom%2Fmpg%2FCHPC0MPG0001M100")?;

            info!("Try login");
            tab.wait_for_element("#input01")?
                .focus()?
                .type_into(&config.user_id)?;
            tab.find_element("#input02")?
                .focus()?
                .type_into(&config.password)?;
            tab.find_elements("#btn_wrap")?.click()?;
            info!("Wait page transition");
            tab.wait_for_element(".name_wrap")?;

            info!("login success");

            info!("open reservation page");
            tab.wait_for_element(".link_reservation")?.click()?;

            info!("open international air");
            tab.wait_for_element(".fx-cobrand-air")?.click()?;

            tab.wait_for_element(".panel.selected table tbody")?;

            let reservation_items = tab.find_elements(".panel.selected table tbody tr")?;

            let codes = reservation_items.iter().map(|item| item.find_element(".txl a").and_then(|elem| elem.get_inner_text())).collect::<Result<_, _>>()?;

            for (reservation_idx, code) in codes.into_iter().enumerate() {
                tab.find_element(format!(".panel.selected table tbody tr:nth-child({})", reservation_idx + 1))?.click();
                let details = tab.wait_for_elements(".flight_detail")?;
                for (trip_idx, detail) in details.into_iter().enumerate() {
                    for (flight_idx, flight) in detail.find_elements(".path li") {
                        let is_wait_element = flight.get_attribute_value("class")?.map(|class_value| class_value.contains("wait")).unwrap_or_default();
                        let day_regex = regex!(r"(\d+)/(\d+)");
                        let start_day = flight.find_element(".start_day")?.get_inner_text()?;
                        let start_day = day_regex.captures(start_day).with_context(|| format!("Failed to parse start day - {start_day}")).and_then(|cap| {
                            let month: u32 = cap.get(0).context("month not found")?.parse().context("Failed to parse month")?;
                            let day: u32 = cap.get(1).context("day not found")?.parse().context("Failed to parse day")?;
                            chrono::NaiveDate::from_ymd_opt(hour, month, day).context("Invalid date")
                        })?;
                        let time_regex = regex!(r"(\d+):(\d+)");
                        let time = flight.find_element(".time")?.get_inner_text()?;
                        let time = time_regex.captures(time).with_context(|| format!("Failed to parse time - {time}")).and_then(|cap| {
                            let hour: u32 = cap.get(0).context("hour not found")?.parse().context("Failed to parse hour")?;
                            let minute: u32 = cap.get(1).context("minute not found")?.parse().context("Failed to parse hour")?;
                            chrono::NaiveTime::from_hms_opt(hour, minute, 0).context("Invalid time")
                        })?;
                        let terminal = flight.find_element(".terminal")?.get_inner_text()?;
                        let loading_time = flight.find_element(".loading_time")?.get_inner_text()?;
                    }
                }
                tab.evaluate("history.back()")?;
            }

            Ok(items)
        })
        .await
        .map_err(|e| anyhow::anyhow!("join error - {e:?}")),
    )?;

    Ok(())
}
