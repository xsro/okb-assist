from __future__ import annotations

from collections.abc import Mapping
from io import BytesIO
from typing import Any, TypeVar, cast

from attrs import define as _attrs_define
from attrs import field as _attrs_field

from .. import types
from ..types import UNSET, File, Unset

T = TypeVar("T", bound="BodyFileParseFileParsePost")


@_attrs_define
class BodyFileParseFileParsePost:
    """
    Attributes:
        files (list[File]): Upload PDF, image, DOCX, PPTX, or XLSX files for parsing
        lang_list (list[str] | Unset): (Adapted only for pipeline and hybrid backend)Input the languages in the pdf to
            improve OCR accuracy.Options:
            - ch: Chinese, English, Chinese Traditional.
            - ch_lite: Chinese, English, Chinese Traditional, Japanese.
            - ch_server: Chinese, English, Chinese Traditional, Japanese.
            - en: English.
            - korean: Korean, English.
            - japan: Chinese, English, Chinese Traditional, Japanese.
            - chinese_cht: Chinese, English, Chinese Traditional, Japanese.
            - ta: Tamil, English.
            - te: Telugu, English.
            - ka: Kannada.
            - th: Thai, English.
            - el: Greek, English.
            - latin: French, German, Afrikaans, Italian, Spanish, Bosnian, Portuguese, Czech, Welsh, Danish, Estonian,
            Irish, Croatian, Uzbek, Hungarian, Serbian (Latin), Indonesian, Occitan, Icelandic, Lithuanian, Maori, Malay,
            Dutch, Norwegian, Polish, Slovak, Slovenian, Albanian, Swedish, Swahili, Tagalog, Turkish, Latin, Azerbaijani,
            Kurdish, Latvian, Maltese, Pali, Romanian, Vietnamese, Finnish, Basque, Galician, Luxembourgish, Romansh,
            Catalan, Quechua.
            - arabic: Arabic, Persian, Uyghur, Urdu, Pashto, Kurdish, Sindhi, Balochi, English.
            - east_slavic: Russian, Belarusian, Ukrainian, English.
            - cyrillic: Russian, Belarusian, Ukrainian, Serbian (Cyrillic), Bulgarian, Mongolian, Abkhazian, Adyghe,
            Kabardian, Avar, Dargin, Ingush, Chechen, Lak, Lezgin, Tabasaran, Kazakh, Kyrgyz, Tajik, Macedonian, Tatar,
            Chuvash, Bashkir, Malian, Moldovan, Udmurt, Komi, Ossetian, Buryat, Kalmyk, Tuvan, Sakha, Karakalpak, English.
            - devanagari: Hindi, Marathi, Nepali, Bihari, Maithili, Angika, Bhojpuri, Magahi, Santali, Newari, Konkani,
            Sanskrit, Haryanvi, English.
        backend (str | Unset): The backend for parsing:
            - pipeline: More general, supports multiple languages, hallucination-free.
            - vlm-auto-engine: High accuracy via local computing power, supports Chinese and English documents only.
            - vlm-http-client: High accuracy via remote computing power(client suitable for openai-compatible servers),
            supports Chinese and English documents only.
            - hybrid-auto-engine: Next-generation high accuracy solution via local computing power, supports multiple
            languages.
            - hybrid-http-client: High accuracy via remote computing power but requires a little local computing
            power(client suitable for openai-compatible servers), supports multiple languages. Default: 'hybrid-auto-
            engine'.
        parse_method (str | Unset): (Adapted only for pipeline and hybrid backend)The method for parsing PDF:
            - auto: Automatically determine the method based on the file type
            - txt: Use text extraction method
            - ocr: Use OCR method for image-based PDFs
             Default: 'auto'.
        formula_enable (bool | Unset): Enable formula parsing. Default: True.
        table_enable (bool | Unset): Enable table parsing. Default: True.
        image_analysis (bool | Unset): Enable image/chart analysis for VLM and hybrid backends. Default: True.
        server_url (None | str | Unset): (Adapted only for <vlm/hybrid>-http-client backend)openai compatible server
            url, e.g., http://127.0.0.1:30000
        return_md (bool | Unset): Return markdown content in response Default: True.
        return_middle_json (bool | Unset): Return middle JSON in response Default: False.
        return_model_output (bool | Unset): Return model output JSON in response Default: False.
        return_content_list (bool | Unset): Return content list JSON in response Default: False.
        return_images (bool | Unset): Return extracted images in response Default: False.
        response_format_zip (bool | Unset): Return results as a ZIP file instead of JSON Default: False.
        return_original_file (bool | Unset): Include the processed original input file in the ZIP result; ignored unless
            response_format_zip=true Default: False.
        client_side_output_generation (bool | Unset): Defer final markdown/content-list generation to the client. When
            enabled, the server returns staged middle JSON, model output, and images. Default: False.
        start_page_id (int | Unset): The starting page for PDF parsing, beginning from 0 Default: 0.
        end_page_id (int | Unset): The ending page for PDF parsing, beginning from 0 Default: 99999.
    """

    files: list[File]
    lang_list: list[str] | Unset = UNSET
    backend: str | Unset = "hybrid-auto-engine"
    parse_method: str | Unset = "auto"
    formula_enable: bool | Unset = True
    table_enable: bool | Unset = True
    image_analysis: bool | Unset = True
    server_url: None | str | Unset = UNSET
    return_md: bool | Unset = True
    return_middle_json: bool | Unset = False
    return_model_output: bool | Unset = False
    return_content_list: bool | Unset = False
    return_images: bool | Unset = False
    response_format_zip: bool | Unset = False
    return_original_file: bool | Unset = False
    client_side_output_generation: bool | Unset = False
    start_page_id: int | Unset = 0
    end_page_id: int | Unset = 99999
    additional_properties: dict[str, Any] = _attrs_field(init=False, factory=dict)

    def to_dict(self) -> dict[str, Any]:
        files = []
        for files_item_data in self.files:
            files_item = files_item_data.to_tuple()

            files.append(files_item)

        lang_list: list[str] | Unset = UNSET
        if not isinstance(self.lang_list, Unset):
            lang_list = self.lang_list

        backend = self.backend

        parse_method = self.parse_method

        formula_enable = self.formula_enable

        table_enable = self.table_enable

        image_analysis = self.image_analysis

        server_url: None | str | Unset
        if isinstance(self.server_url, Unset):
            server_url = UNSET
        else:
            server_url = self.server_url

        return_md = self.return_md

        return_middle_json = self.return_middle_json

        return_model_output = self.return_model_output

        return_content_list = self.return_content_list

        return_images = self.return_images

        response_format_zip = self.response_format_zip

        return_original_file = self.return_original_file

        client_side_output_generation = self.client_side_output_generation

        start_page_id = self.start_page_id

        end_page_id = self.end_page_id

        field_dict: dict[str, Any] = {}
        field_dict.update(self.additional_properties)
        field_dict.update(
            {
                "files": files,
            }
        )
        if lang_list is not UNSET:
            field_dict["lang_list"] = lang_list
        if backend is not UNSET:
            field_dict["backend"] = backend
        if parse_method is not UNSET:
            field_dict["parse_method"] = parse_method
        if formula_enable is not UNSET:
            field_dict["formula_enable"] = formula_enable
        if table_enable is not UNSET:
            field_dict["table_enable"] = table_enable
        if image_analysis is not UNSET:
            field_dict["image_analysis"] = image_analysis
        if server_url is not UNSET:
            field_dict["server_url"] = server_url
        if return_md is not UNSET:
            field_dict["return_md"] = return_md
        if return_middle_json is not UNSET:
            field_dict["return_middle_json"] = return_middle_json
        if return_model_output is not UNSET:
            field_dict["return_model_output"] = return_model_output
        if return_content_list is not UNSET:
            field_dict["return_content_list"] = return_content_list
        if return_images is not UNSET:
            field_dict["return_images"] = return_images
        if response_format_zip is not UNSET:
            field_dict["response_format_zip"] = response_format_zip
        if return_original_file is not UNSET:
            field_dict["return_original_file"] = return_original_file
        if client_side_output_generation is not UNSET:
            field_dict["client_side_output_generation"] = client_side_output_generation
        if start_page_id is not UNSET:
            field_dict["start_page_id"] = start_page_id
        if end_page_id is not UNSET:
            field_dict["end_page_id"] = end_page_id

        return field_dict

    def to_multipart(self) -> types.RequestFiles:
        files: types.RequestFiles = []

        for files_item_element in self.files:
            files.append(("files", files_item_element.to_tuple()))

        if not isinstance(self.lang_list, Unset):
            for lang_list_item_element in self.lang_list:
                files.append(("lang_list", (None, str(lang_list_item_element).encode(), "text/plain")))

        if not isinstance(self.backend, Unset):
            files.append(("backend", (None, str(self.backend).encode(), "text/plain")))

        if not isinstance(self.parse_method, Unset):
            files.append(("parse_method", (None, str(self.parse_method).encode(), "text/plain")))

        if not isinstance(self.formula_enable, Unset):
            files.append(("formula_enable", (None, str(self.formula_enable).encode(), "text/plain")))

        if not isinstance(self.table_enable, Unset):
            files.append(("table_enable", (None, str(self.table_enable).encode(), "text/plain")))

        if not isinstance(self.image_analysis, Unset):
            files.append(("image_analysis", (None, str(self.image_analysis).encode(), "text/plain")))

        if not isinstance(self.server_url, Unset):
            if isinstance(self.server_url, str):
                files.append(("server_url", (None, str(self.server_url).encode(), "text/plain")))
            else:
                files.append(("server_url", (None, str(self.server_url).encode(), "text/plain")))

        if not isinstance(self.return_md, Unset):
            files.append(("return_md", (None, str(self.return_md).encode(), "text/plain")))

        if not isinstance(self.return_middle_json, Unset):
            files.append(("return_middle_json", (None, str(self.return_middle_json).encode(), "text/plain")))

        if not isinstance(self.return_model_output, Unset):
            files.append(("return_model_output", (None, str(self.return_model_output).encode(), "text/plain")))

        if not isinstance(self.return_content_list, Unset):
            files.append(("return_content_list", (None, str(self.return_content_list).encode(), "text/plain")))

        if not isinstance(self.return_images, Unset):
            files.append(("return_images", (None, str(self.return_images).encode(), "text/plain")))

        if not isinstance(self.response_format_zip, Unset):
            files.append(("response_format_zip", (None, str(self.response_format_zip).encode(), "text/plain")))

        if not isinstance(self.return_original_file, Unset):
            files.append(("return_original_file", (None, str(self.return_original_file).encode(), "text/plain")))

        if not isinstance(self.client_side_output_generation, Unset):
            files.append(
                (
                    "client_side_output_generation",
                    (None, str(self.client_side_output_generation).encode(), "text/plain"),
                )
            )

        if not isinstance(self.start_page_id, Unset):
            files.append(("start_page_id", (None, str(self.start_page_id).encode(), "text/plain")))

        if not isinstance(self.end_page_id, Unset):
            files.append(("end_page_id", (None, str(self.end_page_id).encode(), "text/plain")))

        for prop_name, prop in self.additional_properties.items():
            files.append((prop_name, (None, str(prop).encode(), "text/plain")))

        return files

    @classmethod
    def from_dict(cls: type[T], src_dict: Mapping[str, Any]) -> T:
        d = dict(src_dict)
        files = []
        _files = d.pop("files")
        for files_item_data in _files:
            files_item = File(payload=BytesIO(files_item_data))

            files.append(files_item)

        lang_list = cast(list[str], d.pop("lang_list", UNSET))

        backend = d.pop("backend", UNSET)

        parse_method = d.pop("parse_method", UNSET)

        formula_enable = d.pop("formula_enable", UNSET)

        table_enable = d.pop("table_enable", UNSET)

        image_analysis = d.pop("image_analysis", UNSET)

        def _parse_server_url(data: object) -> None | str | Unset:
            if data is None:
                return data
            if isinstance(data, Unset):
                return data
            return cast(None | str | Unset, data)

        server_url = _parse_server_url(d.pop("server_url", UNSET))

        return_md = d.pop("return_md", UNSET)

        return_middle_json = d.pop("return_middle_json", UNSET)

        return_model_output = d.pop("return_model_output", UNSET)

        return_content_list = d.pop("return_content_list", UNSET)

        return_images = d.pop("return_images", UNSET)

        response_format_zip = d.pop("response_format_zip", UNSET)

        return_original_file = d.pop("return_original_file", UNSET)

        client_side_output_generation = d.pop("client_side_output_generation", UNSET)

        start_page_id = d.pop("start_page_id", UNSET)

        end_page_id = d.pop("end_page_id", UNSET)

        body_file_parse_file_parse_post = cls(
            files=files,
            lang_list=lang_list,
            backend=backend,
            parse_method=parse_method,
            formula_enable=formula_enable,
            table_enable=table_enable,
            image_analysis=image_analysis,
            server_url=server_url,
            return_md=return_md,
            return_middle_json=return_middle_json,
            return_model_output=return_model_output,
            return_content_list=return_content_list,
            return_images=return_images,
            response_format_zip=response_format_zip,
            return_original_file=return_original_file,
            client_side_output_generation=client_side_output_generation,
            start_page_id=start_page_id,
            end_page_id=end_page_id,
        )

        body_file_parse_file_parse_post.additional_properties = d
        return body_file_parse_file_parse_post

    @property
    def additional_keys(self) -> list[str]:
        return list(self.additional_properties.keys())

    def __getitem__(self, key: str) -> Any:
        return self.additional_properties[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.additional_properties[key] = value

    def __delitem__(self, key: str) -> None:
        del self.additional_properties[key]

    def __contains__(self, key: str) -> bool:
        return key in self.additional_properties
