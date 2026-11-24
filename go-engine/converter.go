package main

import (
	"net/url"
	"strings"
	"unicode/utf8"

	"github.com/PuerkitoBio/goquery"
	md "github.com/firecrawl/html-to-markdown"
	"github.com/firecrawl/html-to-markdown/plugin"
	"golang.org/x/net/html"
)

// Converter handles HTML cleaning, metadata extraction, and HTML-to-Markdown conversion
type Converter struct {
	converter *md.Converter
}

// NewConverter creates a new Converter instance with pre-configured GFM and code rules
func NewConverter() *Converter {
	converter := md.NewConverter("", true, nil)
	converter.Use(plugin.GitHubFlavored())
	addGenericPreRule(converter)

	return &Converter{
		converter: converter,
	}
}

// ConvertHTMLToMarkdown converts raw HTML string directly to Markdown
func (c *Converter) ConvertHTMLToMarkdown(htmlStr string) (string, error) {
	return c.converter.ConvertString(htmlStr)
}

// CleanAndConvertHTML parses HTML using goquery, extracts metadata and resolved links,
// cleans noise/boilerplate tags, applies include/exclude CSS selectors, and converts to GFM Markdown.
func (c *Converter) CleanAndConvertHTML(
	rawHTML string,
	onlyMainContent bool,
	includeTags []string,
	excludeTags []string,
	baseURL string,
) (markdown string, cleanedHTML string, title string, description string, links []string, err error) {
	links = []string{}

	doc, err := goquery.NewDocumentFromReader(strings.NewReader(rawHTML))
	if err != nil {
		return "", "", "", "", links, err
	}

	// 1. Extract Title (<title> with fallback to <meta property="og:title">)
	title = strings.TrimSpace(doc.Find("title").First().Text())
	var ogTitle, metaDesc, ogDesc string
	doc.Find("meta").Each(func(_ int, s *goquery.Selection) {
		name := strings.ToLower(strings.TrimSpace(s.AttrOr("name", "")))
		prop := strings.ToLower(strings.TrimSpace(s.AttrOr("property", "")))
		content := strings.TrimSpace(s.AttrOr("content", ""))
		if content == "" {
			return
		}
		if name == "description" && metaDesc == "" {
			metaDesc = content
		}
		if prop == "og:title" && ogTitle == "" {
			ogTitle = content
		}
		if prop == "og:description" && ogDesc == "" {
			ogDesc = content
		}
	})
	if title == "" {
		title = ogTitle
	}
	description = metaDesc
	if description == "" {
		description = ogDesc
	}

	// 2. Parse baseURL and extract + deduplicate http/https links
	var parsedBase *url.URL
	if baseURL != "" {
		parsedBase, _ = url.Parse(baseURL)
	}

	seenLinks := make(map[string]struct{})
	doc.Find("a[href]").Each(func(_ int, s *goquery.Selection) {
		href := strings.TrimSpace(s.AttrOr("href", ""))
		if href == "" || strings.HasPrefix(href, "#") || strings.HasPrefix(strings.ToLower(href), "javascript:") || strings.HasPrefix(strings.ToLower(href), "mailto:") || strings.HasPrefix(strings.ToLower(href), "tel:") {
			return
		}
		resolved := resolveURL(parsedBase, href)
		if strings.HasPrefix(resolved, "http://") || strings.HasPrefix(resolved, "https://") {
			s.SetAttr("href", resolved)
			if _, exists := seenLinks[resolved]; !exists {
				seenLinks[resolved] = struct{}{}
				links = append(links, resolved)
			}
		}
	})

	// Also resolve relative image sources against baseURL for cleaner Markdown output
	if parsedBase != nil {
		doc.Find("img[src]").Each(func(_ int, s *goquery.Selection) {
			src := strings.TrimSpace(s.AttrOr("src", ""))
			if src != "" && !strings.HasPrefix(src, "data:") {
				s.SetAttr("src", resolveURL(parsedBase, src))
			}
		})
	}

	// 3. Remove non-content elements
	doc.Find("script, style, noscript, iframe, svg").Remove()

	// 4. Strip boilerplate / navigation / ads when onlyMainContent is enabled
	if onlyMainContent {
		doc.Find("nav, footer, header, aside, .cookie-banner, .ads, #cookie-banner, .cookie-consent, .advertisement").Remove()
	}

	// 5. Apply custom excludeTags CSS selectors
	for _, sel := range excludeTags {
		sel = strings.TrimSpace(sel)
		if sel != "" {
			doc.Find(sel).Remove()
		}
	}

	// 6. Apply custom includeTags CSS selectors if non-empty
	if len(includeTags) > 0 {
		var includedHTML strings.Builder
		for _, sel := range includeTags {
			sel = strings.TrimSpace(sel)
			if sel == "" {
				continue
			}
			doc.Find(sel).Each(func(_ int, s *goquery.Selection) {
				if outer, err := goquery.OuterHtml(s); err == nil {
					includedHTML.WriteString(outer)
					includedHTML.WriteString("\n")
				}
			})
		}
		body := doc.Find("body")
		if body.Length() > 0 {
			body.SetHtml(includedHTML.String())
		} else {
			doc, err = goquery.NewDocumentFromReader(strings.NewReader("<html><body>" + includedHTML.String() + "</body></html>"))
			if err != nil {
				return "", "", title, description, links, err
			}
		}
	}

	// 7. Extract cleaned HTML
	bodySel := doc.Find("body")
	if bodySel.Length() > 0 {
		cleanedHTML, err = bodySel.Html()
		if err != nil {
			cleanedHTML, _ = doc.Html()
		}
	} else {
		cleanedHTML, _ = doc.Html()
	}
	cleanedHTML = strings.TrimSpace(cleanedHTML)

	// 8. Convert cleaned HTML to GitHub-Flavored Markdown
	mdOut, err := c.converter.ConvertString(cleanedHTML)
	if err != nil {
		return "", cleanedHTML, title, description, links, err
	}
	markdown = strings.TrimSpace(mdOut)

	return markdown, cleanedHTML, title, description, links, nil
}

// CleanAndConvertHTML is a package-level helper using a default Converter instance
func CleanAndConvertHTML(
	rawHTML string,
	onlyMainContent bool,
	includeTags []string,
	excludeTags []string,
	baseURL string,
) (markdown string, cleanedHTML string, title string, description string, links []string, err error) {
	return NewConverter().CleanAndConvertHTML(rawHTML, onlyMainContent, includeTags, excludeTags, baseURL)
}

func resolveURL(base *url.URL, ref string) string {
	parsedRef, err := url.Parse(ref)
	if err != nil {
		return ref
	}
	if base != nil {
		return base.ResolveReference(parsedRef).String()
	}
	return parsedRef.String()
}

// addGenericPreRule adds a robust PRE handler that extracts nested code text
// (e.g., tables/rows/gutters) and outputs fenced blocks with detected language.
func addGenericPreRule(conv *md.Converter) {
	isGutter := func(class string) bool {
		c := strings.ToLower(class)
		return strings.Contains(c, "gutter") || strings.Contains(c, "line-numbers")
	}

	detectLang := func(sel *goquery.Selection) string {
		classes := sel.AttrOr("class", "")
		lower := strings.ToLower(classes)
		for _, part := range strings.Fields(lower) {
			if strings.HasPrefix(part, "language-") {
				return strings.TrimPrefix(part, "language-")
			}
			if strings.HasPrefix(part, "lang-") {
				return strings.TrimPrefix(part, "lang-")
			}
		}
		return ""
	}

	// Collect text recursively; insert newlines after block elements and br
	var collect func(n *html.Node, b *strings.Builder)
	collect = func(n *html.Node, b *strings.Builder) {
		if n == nil {
			return
		}
		switch n.Type {
		case html.TextNode:
			b.WriteString(n.Data)
		case html.ElementNode:
			name := strings.ToLower(n.Data)
			// Skip gutters
			if name != "" {
				for _, a := range n.Attr {
					if a.Key == "class" && isGutter(a.Val) {
						return
					}
				}
			}

			if name == "br" {
				b.WriteString("\n")
			}

			for c := n.FirstChild; c != nil; c = c.NextSibling {
				collect(c, b)
			}

			// Newline after block-ish wrappers to preserve lines
			switch name {
			case "p", "div", "li", "tr", "table", "thead", "tbody", "tfoot", "section", "article", "blockquote", "pre", "h1", "h2", "h3", "h4", "h5", "h6":
				b.WriteString("\n")
			}
		}
	}

	conv.AddRules(md.Rule{
		Filter: []string{"pre"},
		Replacement: func(_ string, selec *goquery.Selection, opt *md.Options) *string {
			codeSel := selec.Find("code").First()
			lang := detectLang(codeSel)
			if lang == "" {
				lang = detectLang(selec)
			}

			var b strings.Builder
			for _, n := range selec.Nodes {
				collect(n, &b)
			}
			content := strings.TrimRight(b.String(), "\n")

			fenceChar, _ := utf8.DecodeRuneInString(opt.Fence)
			fence := md.CalculateCodeFence(fenceChar, content)
			text := "\n\n" + fence + lang + "\n" + content + "\n" + fence + "\n\n"
			return md.String(text)
		},
	})

	// Inline code: robustly extract text and fence with backticks
	conv.AddRules(md.Rule{
		Filter: []string{"code"},
		Replacement: func(_ string, selec *goquery.Selection, opt *md.Options) *string {
			if selec.ParentsFiltered("pre").Length() > 0 {
				return nil
			}
			var b strings.Builder
			for _, n := range selec.Nodes {
				collect(n, &b)
			}
			code := b.String()
			code = md.TrimTrailingSpaces(strings.ReplaceAll(code, "\r\n", "\n"))

			fence := "`"
			if strings.Contains(code, "`") {
				fence = "``"
				if strings.Contains(code, "``") {
					fence = "```"
				}
			}
			out := fence + code + fence
			return md.String(out)
		},
	})
}
