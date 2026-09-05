//! Translate server-owned mail keys at the wire boundary, including queued
//! messages created before localization was added. User-authored text is untouched.
pub(crate) fn resolve<'a>(raw: &'a str, locale: &str) -> &'a str {
    if !raw.starts_with("memorial.") { return raw; }
    let (subject, body, cat) = match super::normalized_notice_locale(locale) {
        "zh-Hans" => ("纪念版礼物", "请领取附件中的奖励。每封邮件包含一项奖励。", "喵喵活动奖励"),
        "zh-Hant" => ("紀念版禮物", "請領取附件中的獎勵。每封郵件包含一項獎勵。", "喵喵活動獎勵"),
        "ko" => ("메모리얼 선물", "첨부된 보상을 받으세요. 우편 한 통에 보상 하나가 들어 있습니다.", "고양이 이벤트 보상"),
        "ja" | "jp" => ("メモリアルギフト", "添付の報酬を受け取ってください。メール1通につき報酬1つです。", "にゃんこイベント報酬"),
        "vi" => ("Quà kỷ niệm", "Hãy nhận phần thưởng đính kèm. Mỗi thư có một phần thưởng.", "Thưởng sự kiện mèo"),
        "es" => ("Regalo conmemorativo", "Recoge la recompensa adjunta. Cada mensaje contiene una recompensa.", "Recompensa del evento felino"),
        "it" => ("Regalo commemorativo", "Ritira la ricompensa allegata. Ogni messaggio contiene una ricompensa.", "Ricompensa dell'evento dei gatti"),
        "id" => ("Hadiah memorial", "Ambil hadiah terlampir. Setiap surat berisi satu hadiah.", "Hadiah acara kucing"),
        "th" => ("ของขวัญฉบับอนุสรณ์", "รับรางวัลที่แนบมา จดหมายแต่ละฉบับมีรางวัลหนึ่งรายการ", "รางวัลกิจกรรมแมว"),
        "pt" => ("Presente comemorativo", "Resgate a recompensa anexada. Cada mensagem contém uma recompensa.", "Recompensa do evento dos gatos"),
        "hi" => ("स्मृति उपहार", "संलग्न पुरस्कार प्राप्त करें। प्रत्येक संदेश में एक पुरस्कार है।", "बिल्ली कार्यक्रम का पुरस्कार"),
        _ => ("Memorial gift", "Claim the attached reward. Each message contains one reward.", "Cat event reward"),
    };
    if raw == "memorial.samseck.subject" { cat }
    else if raw.ends_with(".subject") { subject }
    else if raw.ends_with(".body") { body }
    else { raw }
}

#[cfg(test)]
mod tests {
    #[test]
    fn chinese_mail_keys_accept_both_client_and_android_locale_names() {
        for locale in ["zh-Hans", "zh_chs", "zh_CN", "zh-SG"] {
            assert_eq!(super::resolve("memorial.samseck.subject", locale), "喵喵活动奖励");
            assert_eq!(super::resolve("memorial.currency.body", locale), "请领取附件中的奖励。每封邮件包含一项奖励。");
        }
        for locale in ["zh-Hant", "zh_cht", "zh_TW", "zh-HK"] {
            assert_eq!(super::resolve("memorial.samseck.subject", locale), "喵喵活動獎勵");
        }
    }
    #[test]
    fn no_server_key_leaks_for_any_supported_mail_language() {
        for locale in ["ko","en","jp","zh_chs","zh_cht","vi","es","it","id","th","pt","hi","unknown"] {
            for key in ["memorial.samseck.subject","memorial.samseck.body","memorial.legacy.subject","memorial.currency.body"] {
                assert!(!super::resolve(key, locale).starts_with("memorial."));
            }
            assert_eq!(super::resolve("Personal message",locale), "Personal message");
        }
    }
}
